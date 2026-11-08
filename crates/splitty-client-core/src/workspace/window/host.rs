//! Immutable host action captures and bounded presentation readers. Actual
//! preparation/read/cleanup futures remain in the native worker.
use super::*;
use splitty_transport::{budget::Charged, wire::Progress};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostActionTarget {
    pub(super) local_owner: SourceToken,
    pub(super) target: String,
    pub(super) source: SourceId,
    pub(super) attempt: u64,
}
impl HostActionTarget {
    pub fn local_owner(&self) -> SourceToken {
        self.local_owner
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn source_id(&self) -> SourceId {
        self.source
    }
    pub fn attempt(&self) -> u64 {
        self.attempt
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogState {
    Unrequested,
    Loading,
    Ready,
    Failed,
}

/// Decoded only from a bounded, typed diagnostic. Retryability comes from the
/// outer transport Closed envelope, never from message text or a hint.
pub struct HostDiagnostic {
    pub(crate) code: Option<String>,
    pub(crate) message: String,
    pub(crate) hint: Option<String>,
    pub(crate) retryable: bool,
    pub(crate) update_offer: Option<HostUpdateOffer>,
}
impl HostDiagnostic {
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
    pub fn retryable(&self) -> bool {
        self.retryable
    }
}

/// The window's single host presentation, including pre-Client preparation.
/// Variable progress/error data retains its transport charge across view clones.
#[derive(Clone)]
pub struct HostPresentation {
    pub(super) action: HostActionTarget,
    pub(crate) status: SourceStatus,
    pub(crate) progress: Option<Arc<Charged<Progress>>>,
    pub(crate) diagnostic: Option<Arc<Charged<HostDiagnostic>>>,
    pub(crate) catalog: CatalogState,
    pub(crate) catalog_error: Option<Arc<Charged<HostDiagnostic>>>,
    pub(crate) browsing: bool,
    pub(crate) retry_available: bool,
    pub(crate) update_offer: Option<Arc<Charged<HostUpdateOffer>>>,
    pub(crate) update_state: HostUpdateState,
}
impl HostPresentation {
    pub fn update_offer(&self) -> Option<&HostUpdateOffer> {
        self.update_offer.as_deref().map(|offer| &offer.value)
    }
    pub fn update_state(&self) -> HostUpdateState {
        self.update_state
    }
    pub fn is_update_available(&self) -> bool {
        self.update_offer.is_some()
            && self.update_state == HostUpdateState::Idle
            && (self.status == SourceStatus::Ready || self.can_retry())
    }
    pub fn action_target(&self) -> &HostActionTarget {
        &self.action
    }
    pub fn status(&self) -> SourceStatus {
        self.status
    }
    pub fn progress(&self) -> Option<&Progress> {
        self.progress.as_ref().map(|value| &value.value)
    }
    pub fn diagnostic(&self) -> Option<&HostDiagnostic> {
        self.diagnostic.as_ref().map(|value| &value.value)
    }
    pub fn catalog_state(&self) -> CatalogState {
        self.catalog
    }
    pub fn catalog_error(&self) -> Option<&HostDiagnostic> {
        self.catalog_error.as_ref().map(|value| &value.value)
    }
    pub fn is_browsing(&self) -> bool {
        self.browsing
    }
    pub fn can_retry(&self) -> bool {
        self.status == SourceStatus::Offline
            && self.retry_available
            && self.update_state != HostUpdateState::Closing
    }
    /// Retry only the bounded observation on the existing healthy connection.
    /// This never reconnects SSH or replaces the source attempt.
    pub fn can_retry_catalog(&self) -> bool {
        self.status == SourceStatus::Ready && self.catalog == CatalogState::Failed
    }
    pub fn can_cancel(&self) -> bool {
        self.update_state == HostUpdateState::Closing
            || matches!(
                self.status,
                SourceStatus::Connecting | SourceStatus::Reconnecting
            )
    }
}
