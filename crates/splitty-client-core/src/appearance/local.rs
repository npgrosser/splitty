use super::{theme_name, AppearanceSnapshot, ColorScheme};
use serde::{de::DeserializeOwned, Deserialize};
use splitty_core::{config::AppearanceSettings, theme::ResolvedTheme};
use splitty_server::{native::client::Client, service::catalog::Operation};
use splitty_transport::{budget::Charged, codec, wire};
use std::{future::Future, io, pin::Pin, sync::Arc};
use tokio::sync::Notify;
use uuid::Uuid;

// Existing API bodies have this ceiling. No copy or typed decode is admitted first.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
type Read = Pin<Box<dyn Future<Output = (Client, io::Result<AppearanceSnapshot>)> + Send>>;

/// Owns exactly one API-only stream and one retained request future.
///
/// `next_update` can be cancelled by a worker's select without losing that future
/// or its client. Refreshes coalesce until the current read completes. On owner
/// shutdown call `finish` to cancel a pending request and await stream cleanup.
/// Dropping this value still revokes the stream through the native client's Drop.
pub struct AppearanceReader {
    client: Option<Client>,
    pending: Option<Read>,
    cancel: Arc<Notify>,
    connection_id: Uuid,
    generation: u64,
    scheme: ColorScheme,
    dirty: bool,
}

impl AppearanceReader {
    pub(crate) fn new(client: Client, connection_id: Uuid, scheme: ColorScheme) -> Self {
        Self {
            client: Some(client),
            pending: None,
            cancel: Arc::new(Notify::new()),
            connection_id,
            generation: 1,
            scheme,
            dirty: true,
        }
    }

    /// Called for local settings changes or local OS preference changes.
    /// A remote or replaced session source cannot select application appearance.
    pub fn refresh(&mut self, connection_id: Uuid, scheme: ColorScheme) -> io::Result<()> {
        if connection_id != self.connection_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "appearance source changed",
            ));
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("appearance generation exhausted"))?;
        self.scheme = scheme;
        self.dirty = true;
        Ok(())
    }

    /// Returns only the newest requested snapshot or error. When clean, waits
    /// without spinning until the caller cancels this poll to request a refresh.
    pub async fn next_update(&mut self) -> io::Result<AppearanceSnapshot> {
        loop {
            if self.pending.is_none() {
                if !self.dirty {
                    std::future::pending::<()>().await;
                }
                let mut client = self
                    .client
                    .take()
                    .expect("appearance reader owns its client");
                let cancel = self.cancel.clone();
                let connection = self.connection_id;
                let generation = self.generation;
                let scheme = self.scheme;
                self.dirty = false;
                self.pending = Some(Box::pin(async move {
                    let result = tokio::select! { biased;
                        _ = cancel.notified() => Err(io::Error::new(io::ErrorKind::Interrupted, "appearance read cancelled")),
                        result = read(&mut client, connection, generation, scheme) => result,
                    };
                    (client, result)
                }));
            }
            let (client, result) = self
                .pending
                .as_mut()
                .expect("appearance read started")
                .await;
            self.pending = None;
            self.client = Some(client);
            if self.dirty {
                // The old snapshot/error cannot overwrite a newer local choice.
                continue;
            }
            return result;
        }
    }

    pub async fn finish(mut self) -> io::Result<()> {
        if let Some(pending) = self.pending.take() {
            self.cancel.notify_one();
            let (client, _) = pending.await;
            self.client = Some(client);
        }
        if let Some(client) = self.client.take() {
            client.finish().await?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct HomeAppearance {
    appearance: AppearanceSettings,
}

async fn read(
    client: &mut Client,
    connection_id: Uuid,
    generation: u64,
    scheme: ColorScheme,
) -> io::Result<AppearanceSnapshot> {
    let reply = client
        .request(Operation::HomeGet, serde_json::Map::new())
        .await?;
    let settings = decode::<HomeAppearance>(client, &reply)?.map(|home| home.appearance);
    drop(reply);
    // Theme identifiers are bounded by the existing resolver. Reject oversized
    // configuration strings before allocating request arguments.
    let name = theme_name(&settings, scheme);
    if name.len() > 64 || settings.font_family.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "appearance string exceeds native limit",
        ));
    }
    let mut args = serde_json::Map::new();
    args.insert("name".into(), name.into());
    let reply = client.request(Operation::ThemeGet, args).await?;
    let theme = decode::<ResolvedTheme>(client, &reply)?;
    Ok(AppearanceSnapshot {
        connection_id,
        generation,
        settings,
        theme,
    })
}

fn decode<T: DeserializeOwned>(client: &Client, reply: &wire::Reply) -> io::Result<Charged<T>> {
    if !(200..300).contains(&reply.status) || reply.content_type != wire::ContentType::Json {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("appearance API returned status {}", reply.status),
        ));
    }
    let bytes = reply.body.as_bytes();
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "appearance response exceeds native limit",
        ));
    }
    let sender = client.sender();
    let lease = codec::reserve_json_typed(bytes, &sender.context().budget, false)?;
    let value = serde_json::from_slice(bytes).map_err(|_| {
        // A serde diagnostic can include a peer-supplied string. Do not retain
        // that allocation after this decode's scratch lease is released.
        io::Error::new(io::ErrorKind::InvalidData, "invalid appearance response")
    })?;
    Ok(lease.charge(value))
}

#[cfg(test)]
mod tests;
