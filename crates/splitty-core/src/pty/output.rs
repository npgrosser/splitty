//! Bounded log queries and opaque, generation-bound reader cursors.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};

use super::ringbuffer::{BufferSlice, Selection};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Raw,
}

#[derive(Clone, Debug)]
pub struct OutputQuery {
    pub cursor: Option<String>,
    pub tail: Option<usize>,
    pub limit: usize,
    pub format: LogFormat,
}

impl Default for OutputQuery {
    fn default() -> Self {
        Self {
            cursor: None,
            tail: None,
            limit: 64 * 1024,
            format: LogFormat::Text,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("invalid output cursor")]
    InvalidCursor,
    #[error("invalid output options: {0}")]
    InvalidOptions(&'static str),
    #[error("output is unavailable")]
    Unavailable,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    v: u8,
    job: String,
    generation: String,
    format: LogFormat,
    position: u64,
}

fn encode(job: &str, generation: &str, format: LogFormat, position: u64) -> String {
    let cursor = Cursor {
        v: 1,
        job: job.into(),
        generation: generation.into(),
        format,
        position,
    };
    URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&cursor).expect("cursor has only serializable scalar fields"))
}

impl OutputQuery {
    pub fn validate_options(&self) -> Result<(), OutputError> {
        if self.limit == 0
            || self.limit > 256 * 1024
            || (self.format == LogFormat::Text && self.limit < 4)
        {
            return Err(OutputError::InvalidOptions("invalid limit"));
        }
        if self.tail.is_some_and(|n| n > 10000)
            || (self.tail.is_some() && (self.cursor.is_some() || self.format == LogFormat::Raw))
        {
            return Err(OutputError::InvalidOptions("invalid tail selection"));
        }
        Ok(())
    }
    pub(super) fn selection(&self, job: &str, generation: &str) -> Result<Selection, OutputError> {
        if self.limit == 0
            || self.limit > 256 * 1024
            || (self.format == LogFormat::Text && self.limit < 4)
        {
            return Err(OutputError::InvalidOptions(
                "limit must be 1..262144 bytes (at least 4 for text)",
            ));
        }
        if self.tail.is_some_and(|lines| lines > 10000)
            || (self.tail.is_some() && (self.cursor.is_some() || self.format == LogFormat::Raw))
        {
            return Err(OutputError::InvalidOptions(
                "tail requires text, at most 10000 lines, and no cursor",
            ));
        }
        if let Some(encoded) = &self.cursor {
            if encoded.len() > 2048 {
                return Err(OutputError::InvalidCursor);
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|_| OutputError::InvalidCursor)?;
            let cursor: Cursor =
                serde_json::from_slice(&bytes).map_err(|_| OutputError::InvalidCursor)?;
            if cursor.v != 1
                || cursor.job != job
                || cursor.generation != generation
                || cursor.format != self.format
            {
                return Err(OutputError::InvalidCursor);
            }
            return Ok(Selection::From(cursor.position));
        }
        Ok(if self.format == LogFormat::Text {
            Selection::Tail(self.tail.unwrap_or(100))
        } else {
            Selection::Oldest
        })
    }
}

/// Long-poll without an output-stream subscription. Dropping the future drops
/// its watch receiver; authorization and waiter admission belong to the caller.
pub async fn read_waiting(
    session: &dyn super::SessionHandle,
    job: &str,
    query: &OutputQuery,
    wait: std::time::Duration,
) -> Result<OutputPage, OutputError> {
    if wait > std::time::Duration::from_secs(30) {
        return Err(OutputError::InvalidOptions("wait exceeds 30 seconds"));
    }
    let mut changed = session.output_watch();
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let page = session.read_output(job, query)?;
        if !page.data.is_empty() || page.output_complete || wait.is_zero() {
            return Ok(page);
        }
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return session.read_output(job, query),
            result = changed.changed() => {
                if result.is_err() { return session.read_output(job, query); }
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputPage {
    pub data: String,
    pub encoding: &'static str,
    pub next_cursor: Option<String>,
    pub oldest_cursor: Option<String>,
    pub end_cursor: Option<String>,
    pub lost_bytes: u64,
    pub has_more: bool,
    pub output_complete: bool,
    pub output_available: bool,
}

impl OutputPage {
    pub fn unavailable(format: LogFormat) -> Self {
        Self {
            data: String::new(),
            encoding: if format == LogFormat::Raw {
                "base64"
            } else {
                "utf8"
            },
            next_cursor: None,
            oldest_cursor: None,
            end_cursor: None,
            lost_bytes: 0,
            has_more: false,
            output_complete: true,
            output_available: false,
        }
    }

    pub(super) fn from_slice(
        job: &str,
        generation: &str,
        format: LogFormat,
        slice: BufferSlice,
        complete: bool,
    ) -> Result<Self, OutputError> {
        let data = match format {
            LogFormat::Raw => base64::engine::general_purpose::STANDARD.encode(slice.data),
            LogFormat::Text => {
                String::from_utf8(slice.data).map_err(|_| OutputError::InvalidCursor)?
            }
        };
        Ok(Self {
            data,
            encoding: if format == LogFormat::Raw {
                "base64"
            } else {
                "utf8"
            },
            next_cursor: Some(encode(job, generation, format, slice.next)),
            oldest_cursor: Some(encode(job, generation, format, slice.oldest)),
            end_cursor: Some(encode(job, generation, format, slice.end)),
            lost_bytes: slice.lost,
            has_more: slice.next < slice.end,
            output_complete: complete,
            output_available: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_are_bound_to_job_generation_and_representation() {
        let cursor = encode("job", "generation", LogFormat::Text, 27);
        let mut query = OutputQuery {
            cursor: Some(cursor),
            ..Default::default()
        };
        assert!(matches!(
            query.selection("job", "generation"),
            Ok(Selection::From(27))
        ));
        assert!(matches!(
            query.selection("other", "generation"),
            Err(OutputError::InvalidCursor)
        ));
        assert!(matches!(
            query.selection("job", "next-incarnation"),
            Err(OutputError::InvalidCursor)
        ));
        query.format = LogFormat::Raw;
        assert!(matches!(
            query.selection("job", "generation"),
            Err(OutputError::InvalidCursor)
        ));
        query.cursor = Some("invalid cursor".into());
        assert!(matches!(
            query.selection("job", "generation"),
            Err(OutputError::InvalidCursor)
        ));
    }
}
