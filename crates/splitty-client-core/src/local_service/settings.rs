use super::decode;
use crate::{
    api_stream::ApiStream,
    appearance::local::HomeAppearance,
    settings::{SettingsEdit, SettingsField},
};
use splitty_core::native_api::Operation;
use splitty_transport::wire;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsWriteOutcome {
    /// The service acknowledged the write. Presentation waits for an authoritative
    /// snapshot at or above the result's required refresh generation.
    Confirmed,
    Rejected {
        reason: String,
    },
    Uncertain {
        reason: String,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsWriteResult {
    connection_id: Uuid,
    request_id: Uuid,
    field: SettingsField,
    generation: u64,
    outcome: SettingsWriteOutcome,
}
impl SettingsWriteResult {
    #[cfg(feature = "ui-test-support")]
    #[doc(hidden)]
    pub fn ui_test_fixture(
        connection_id: Uuid,
        request_id: Uuid,
        field: SettingsField,
        generation: u64,
        outcome: SettingsWriteOutcome,
    ) -> Self {
        Self {
            connection_id,
            request_id,
            field,
            generation,
            outcome,
        }
    }
    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }
    pub fn request_id(&self) -> Uuid {
        self.request_id
    }
    pub fn field(&self) -> SettingsField {
        self.field
    }
    /// For an acknowledged write, the minimum fresh snapshot generation required
    /// before completing presentation. The reply never supplies editor values.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn outcome(&self) -> &SettingsWriteOutcome {
        &self.outcome
    }
    pub(crate) fn rejected(
        connection_id: Uuid,
        request_id: Uuid,
        field: SettingsField,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            connection_id,
            request_id,
            field,
            generation: 0,
            outcome: SettingsWriteOutcome::Rejected {
                reason: reason.into(),
            },
        }
    }
    pub(super) fn require_refresh(&mut self, generation: u64) {
        if matches!(self.outcome, SettingsWriteOutcome::Confirmed) {
            self.generation = generation;
        }
    }
}
fn uncertain(reason: &str) -> SettingsWriteOutcome {
    SettingsWriteOutcome::Uncertain {
        reason: reason.into(),
    }
}
fn classify(client: &ApiStream, reply: &wire::Reply) -> SettingsWriteOutcome {
    if reply.uncertain {
        return uncertain("Settings update outcome is unknown; refreshing current values");
    }
    if !(200..300).contains(&reply.status) {
        return SettingsWriteOutcome::Rejected {
            reason: "The local service rejected this setting".into(),
        };
    }
    match decode::<HomeAppearance>(client, reply).and_then(|home| home.validate()) {
        Ok(()) => SettingsWriteOutcome::Confirmed,
        Err(_) => uncertain("Settings update reply was invalid; refreshing current values"),
    }
}
pub(super) async fn execute(
    client: &mut ApiStream,
    owner: Uuid,
    request: Uuid,
    edit: SettingsEdit,
    generation: u64,
    cancel: &Notify,
) -> SettingsWriteResult {
    let outcome = tokio::select! { biased;
        _ = cancel.notified() => uncertain("Settings update cancelled; outcome is unknown"),
        result = client.request(Operation::HomeUpdate, edit.args()) => match result {
            Ok(reply) => classify(client, &reply),
            Err(_) => uncertain("Settings update reply unavailable; outcome is unknown"),
        },
    };
    SettingsWriteResult {
        connection_id: owner,
        request_id: request,
        field: edit.field(),
        generation,
        outcome,
    }
}

/// Successful authoritative read after uncertain writes; it makes the fields
/// editable again without claiming that any particular write was persisted.
#[derive(Clone, Debug)]
pub struct SettingsReconciled {
    pub(super) connection_id: Uuid,
    pub(super) generation: u64,
    pub(super) requests: Vec<(SettingsField, Uuid)>,
}
impl SettingsReconciled {
    #[cfg(feature = "ui-test-support")]
    #[doc(hidden)]
    pub fn ui_test_fixture(
        connection_id: Uuid,
        generation: u64,
        requests: Vec<(SettingsField, Uuid)>,
    ) -> Self {
        Self {
            connection_id,
            generation,
            requests,
        }
    }
    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn requests(&self) -> &[(SettingsField, Uuid)] {
        &self.requests
    }
}
