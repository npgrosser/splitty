//! Client-owned decoding of a hub diagnostic carried inside transport Closed.
use super::{
    host_update::{self, Availability, Migration},
    HostDiagnostic,
};
use serde::{Deserialize, Deserializer};
use splitty_transport::{
    budget::{Charged, ConnectionBudget},
    codec, wire,
};
use std::{io, mem::size_of};

const MAX_CLOSED_MESSAGE: usize = wire::MAX_CONTROL_BYTES;
const MAX_CODE: usize = 64;
const MAX_FIELD: usize = wire::MAX_CONTROL_BYTES;

#[derive(Deserialize)]
struct EnvelopeMarker {
    #[serde(rename = "type", default, deserialize_with = "optional_string")]
    kind: Option<String>,
}

fn optional_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct RemoteError {
    #[serde(rename = "type")]
    kind: String,
    code: String,
    message: String,
    #[serde(default, deserialize_with = "optional_string")]
    hint: Option<String>,
    #[serde(rename = "retryable")]
    _retryable: bool,
    version: Option<String>,
    #[serde(rename = "keepsSessions")]
    keeps_sessions: Option<bool>,
    update: Option<Availability>,
    migration: Option<Migration>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid host diagnostic")
}

impl HostDiagnostic {
    pub(crate) fn from_closed(
        closed: &wire::Closed,
        budget: &ConnectionBudget,
    ) -> io::Result<Charged<Self>> {
        let message = closed.message.as_str();
        if message.len() > MAX_CLOSED_MESSAGE {
            return Err(invalid());
        }
        if message.trim_start().starts_with('{') {
            // Typed error formatting can require roughly 25 times a long
            // peer string. Charge that bound on data capacity before serde
            // allocates, leaving reserved controls available for cancellation.
            let scratch = codec::reserve_json_typed(message.as_bytes(), budget, false)?;
            let marker: EnvelopeMarker = serde_json::from_str(message).map_err(|_| invalid())?;
            let is_remote_error = marker.kind.as_deref() == Some("remote.error");
            drop(marker);
            if !is_remote_error {
                // Valid ordinary JSON text is still a diagnostic, not an
                // authority-bearing remote.error envelope.
                drop(scratch);
                return Self::plain(message, closed.retryable, budget);
            }
            let parsed: RemoteError = serde_json::from_str(message).map_err(|_| invalid())?;
            if parsed.kind != "remote.error"
                || parsed.code.is_empty()
                || parsed.code.len() > MAX_CODE
                || !parsed
                    .code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                || parsed.message.len() > MAX_FIELD
                || parsed
                    .hint
                    .as_ref()
                    .is_some_and(|hint| hint.len() > MAX_FIELD)
            {
                return Err(invalid());
            }
            let update_offer = host_update::from_failure(
                parsed.version,
                parsed.keeps_sessions,
                parsed.update,
                parsed.migration,
            )?;
            if update_offer.is_some() && parsed.code != "incompatible" {
                return Err(invalid());
            }
            let retained = update_offer
                .as_ref()
                .map_or(0, |offer| offer.retained_bytes())
                + size_of::<Self>()
                + parsed.code.capacity()
                + parsed.message.capacity()
                + parsed.hint.as_ref().map_or(0, String::capacity);
            return Ok(scratch.shrink(retained).charge(Self {
                code: Some(parsed.code),
                message: parsed.message,
                hint: parsed.hint,
                retryable: closed.retryable,
                update_offer,
            }));
        }

        Self::plain(message, closed.retryable, budget)
    }

    fn plain(
        message: &str,
        retryable: bool,
        budget: &ConnectionBudget,
    ) -> io::Result<Charged<Self>> {
        let lease = budget
            .reserve(size_of::<Self>() + message.len(), true)
            .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
        let mut copied = String::with_capacity(message.len());
        copied.push_str(message);
        Ok(lease.charge(Self {
            code: None,
            message: copied,
            hint: None,
            retryable,
            update_offer: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splitty_transport::budget::ProcessBudget;
    use std::sync::Arc;

    fn closed(message: impl Into<String>, retryable: bool) -> wire::Closed {
        wire::Closed {
            code: wire::CloseCode::BackendLost,
            message: message.into(),
            retryable,
        }
    }

    #[test]
    fn ordinary_and_legacy_offers_are_typed_and_charged() {
        use serde_json::json;
        let budget = ConnectionBudget::default();
        let base = json!({"type":"remote.error","code":"incompatible","message":"update available","retryable":false,"version":"0.1.0","keepsSessions":true});
        for migration in [false, true] {
            let mut value = base.clone();
            if migration {
                value["migration"] = json!({"available":true,"fromVersion":"0.1.0","toVersion":"0.2.0","sessions":3});
            } else {
                value["update"] = json!({"available":true,"keepsSessions":true});
            }
            let diagnostic =
                HostDiagnostic::from_closed(&closed(value.to_string(), false), &budget).unwrap();
            let offer = diagnostic.update_offer.as_ref().unwrap();
            assert_eq!(offer.from_version(), "0.1.0");
            assert_eq!(offer.legacy_sessions(), migration.then_some(3));
            assert_eq!(offer.to_version(), migration.then_some("0.2.0"));
            assert!(budget.used() > 0);
            drop(diagnostic);
            assert_eq!(budget.used(), 0);
            for (field, replacement) in [
                ("keepsSessions", json!(false)),
                ("version", json!("changed")),
            ] {
                let mut invalid = value.clone();
                invalid[field] = replacement;
                if field == "version" && !migration {
                    invalid[field] = json!("x".repeat(65));
                }
                assert!(
                    HostDiagnostic::from_closed(&closed(invalid.to_string(), false), &budget)
                        .is_err()
                );
                assert_eq!(budget.used(), 0);
            }
        }
        // The hub's plan names the version the host gets.
        let mut named = base.clone();
        named["update"] = json!({"available":true,"keepsSessions":true,"toVersion":"0.4.0"});
        let diagnostic =
            HostDiagnostic::from_closed(&closed(named.to_string(), false), &budget).unwrap();
        let offer = diagnostic.update_offer.as_ref().unwrap();
        assert_eq!(offer.to_version(), Some("0.4.0"));
        assert_eq!(offer.legacy_sessions(), None);
        drop(diagnostic);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn outer_retryability_wins_and_hint_remains_data() {
        let budget = ConnectionBudget::default();
        for (outer, inner) in [(false, true), (true, false)] {
            let payload = format!(
                r#"{{"type":"remote.error","code":"auth","message":"Login failed","hint":"ssh vm; echo $HOME","retryable":{inner},"update":{{"available":false,"keepsSessions":true}}}}"#
            );
            let value = HostDiagnostic::from_closed(&closed(payload, outer), &budget).unwrap();
            assert_eq!(value.code(), Some("auth"));
            assert_eq!(value.message(), "Login failed");
            assert_eq!(value.hint(), Some("ssh vm; echo $HOME"));
            assert_eq!(value.retryable(), outer);
            drop(value);
            assert_eq!(budget.used(), 0);
        }
    }

    #[test]
    fn malformed_or_oversized_structured_diagnostics_do_not_become_plain_text() {
        let budget = ConnectionBudget::default();
        for payload in [
            "{bad json".to_owned(),
            r#"{"type":"remote.error","code":"auth","message":"x","retryable":true,"code":"daemon"}"#.to_owned(),
            r#"{"type":"remote.error","code":7,"message":"x","retryable":true}"#.to_owned(),
            r#"{"type":"remote.error","code":"auth","message":"x","retryable":"yes"}"#.to_owned(),
            r#"{"type":"remote.error","code":"auth","message":"x","hint":null,"retryable":true}"#.to_owned(),
            r#"{"type":null,"message":"x"}"#.to_owned(),
            format!(r#"{{"type":"remote.error","code":"auth","message":"{}","retryable":true}}"#, "x".repeat(MAX_FIELD + 1)),
            format!(r#"{{"type":"remote.error","code":"auth","message":"x","hint":"{}","retryable":true}}"#, "x".repeat(MAX_FIELD + 1)),
        ] {
            assert_eq!(
                HostDiagnostic::from_closed(&closed(payload, true), &budget)
                    .err().unwrap()
                    .kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(budget.used(), 0);
        }
    }

    #[test]
    fn plain_text_is_bounded_without_display_parsing() {
        let budget = ConnectionBudget::default();
        for text in [
            "remote.error: retryable=true",
            "[ssh] connection closed",
            r#"{"type":"other","message":"ordinary JSON diagnostic"}"#,
        ] {
            let value = HostDiagnostic::from_closed(&closed(text, false), &budget).unwrap();
            assert_eq!(value.code(), None);
            assert_eq!(value.message(), text);
            assert!(!value.retryable());
            drop(value);
            assert_eq!(budget.used(), 0);
        }
        assert_eq!(
            HostDiagnostic::from_closed(&closed("x".repeat(MAX_CLOSED_MESSAGE + 1), true), &budget)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn sender_sized_message_and_hint_remain_valid() {
        let budget = ConnectionBudget::default();
        for (message, hint) in [
            ("m".repeat(2500), String::new()),
            ("short".into(), "h".repeat(1500)),
        ] {
            let payload = format!(
                r#"{{"type":"remote.error","code":"daemon","message":"{message}","hint":"{hint}","retryable":false}}"#
            );
            let closed = closed(payload, true);
            splitty_transport::codec::Frame::json(wire::Kind::Closed, &closed).unwrap();
            let value = HostDiagnostic::from_closed(&closed, &budget).unwrap();
            assert_eq!(value.message(), message);
            assert_eq!(value.hint(), Some(hint.as_str()));
            assert!(value.retryable());
            drop(value);
            assert_eq!(budget.used(), 0);
        }
    }

    #[test]
    fn escaped_envelope_type_uses_normal_json_string_semantics() {
        let budget = ConnectionBudget::default();
        let value = HostDiagnostic::from_closed(
            &closed(
                r#"{"type":"remote\u002eerror","code":"daemon","message":"gone","retryable":false}"#,
                true,
            ),
            &budget,
        )
        .unwrap();
        assert_eq!(value.code(), Some("daemon"));
        assert_eq!(value.message(), "gone");
        assert!(value.retryable());
        drop(value);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn returned_charge_survives_presentation_clones_until_final_drop() {
        let budget = ConnectionBudget::default();
        let value = Arc::new(
            HostDiagnostic::from_closed(
                &closed(
                    r#"{"type":"remote.error","code":"daemon","message":"gone","retryable":true}"#,
                    true,
                ),
                &budget,
            )
            .unwrap(),
        );
        let clone = value.clone();
        assert!(budget.data.used() > 0);
        drop(value);
        assert!(budget.data.used() > 0);
        drop(clone);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn scratch_admission_precedes_structured_parse() {
        let process = ProcessBudget::new(8192, 8064);
        let budget = ConnectionBudget::new(process, 8192, 8064);
        assert_eq!(
            HostDiagnostic::from_closed(&closed("{bad json", true), &budget)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::OutOfMemory
        );
        assert_eq!(budget.used(), 0);
        assert_eq!(
            HostDiagnostic::from_closed(&closed("{bad json", true), &ConnectionBudget::default())
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
