//! Bounded hub facts and one captured native confirmation, without UI wording
//! or a second copy of the backend's replacement policy.
use super::{HostActionTarget, SourceToken};
use serde::Deserialize;
use splitty_transport::budget::{Charged, ConnectionBudget, Lease};
use std::{io, sync::Arc};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostUpdateOffer {
    from_version: String,
    to_version: Option<String>,
    legacy_sessions: Option<u32>,
}
impl HostUpdateOffer {
    pub fn from_version(&self) -> &str {
        &self.from_version
    }
    pub fn to_version(&self) -> Option<&str> {
        self.to_version.as_deref()
    }
    pub fn legacy_sessions(&self) -> Option<u32> {
        self.legacy_sessions
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.from_version.capacity()
            + self.to_version.as_ref().map_or(0, String::capacity)
    }
    pub(crate) fn from_ready(
        value: &serde_json::Value,
        budget: &ConnectionBudget,
    ) -> io::Result<Option<Arc<Charged<Self>>>> {
        // Read borrowed bounded metadata; do not copy the ready screen directory.
        if value.get("update").is_none() {
            return Ok(None);
        }
        let update: Availability =
            Availability::deserialize(&value["update"]).map_err(|_| invalid())?;
        if !update.available {
            return Ok(None);
        }
        let version = value["daemon"]["version"].as_str().ok_or_else(invalid)?;
        if !update.keeps_sessions
            || value["daemon"]["keepsSessions"] != true
            || !valid_version(version)
        {
            return Err(invalid());
        }
        let lease = budget
            .reserve(std::mem::size_of::<Self>() + version.len(), false)
            .map_err(io::Error::other)?;
        Ok(Some(Arc::new(lease.charge(Self {
            from_version: version.into(),
            to_version: None,
            legacy_sessions: None,
        }))))
    }
}

/// The version the host's daemon reported in `remote.ready`, kept for display
/// only. A missing or malformed value, or an exhausted budget, is no version;
/// it never fails the connection.
pub(crate) fn daemon_version_from_ready(
    value: &serde_json::Value,
    budget: &ConnectionBudget,
) -> Option<Arc<Charged<String>>> {
    let version = value["daemon"]["version"]
        .as_str()
        .filter(|version| valid_version(version))?;
    let lease = budget.reserve(version.len(), false).ok()?;
    Some(Arc::new(lease.charge(version.to_owned())))
}

pub(crate) fn valid_version(version: &str) -> bool {
    !version.is_empty() && version.len() <= 64 && !version.chars().any(char::is_control)
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid host update offer")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Availability {
    available: bool,
    keeps_sessions: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Migration {
    available: bool,
    from_version: String,
    to_version: String,
    sessions: u32,
}
pub(super) fn from_failure(
    version: Option<String>,
    keeps: Option<bool>,
    update: Option<Availability>,
    migration: Option<Migration>,
) -> io::Result<Option<HostUpdateOffer>> {
    if let Some(migration) = migration {
        if !migration.available
            || keeps != Some(true)
            || update.is_some()
            || !valid_version(&migration.from_version)
            || !valid_version(&migration.to_version)
            || version.as_deref() != Some(&migration.from_version)
        {
            return Err(invalid());
        }
        return Ok(Some(HostUpdateOffer {
            from_version: migration.from_version,
            to_version: Some(migration.to_version),
            legacy_sessions: Some(migration.sessions),
        }));
    }
    let Some(update) = update.filter(|update| update.available) else {
        return Ok(None);
    };
    let version = version.ok_or_else(invalid)?;
    if !valid_version(&version) || !update.keeps_sessions || keeps != Some(true) {
        return Err(invalid());
    }
    Ok(Some(HostUpdateOffer {
        from_version: version,
        to_version: None,
        legacy_sessions: None,
    }))
}

/// Immutable confirmation owned by the native worker. Clones retain the same
/// charged facts; possession alone does not authorize execution.
#[derive(Debug)]
pub struct HostUpdatePlan {
    request_id: Uuid,
    target: HostActionTarget,
    offer: Arc<Charged<HostUpdateOffer>>,
    replacement: String,
    _allocation: Lease,
}
impl HostUpdatePlan {
    pub fn request_id(&self) -> Uuid {
        self.request_id
    }
    pub fn target(&self) -> &HostActionTarget {
        &self.target
    }
    pub fn owner(&self) -> SourceToken {
        self.target.local_owner()
    }
    pub fn from_version(&self) -> &str {
        self.offer.from_version()
    }
    pub fn to_version(&self) -> &str {
        &self.replacement
    }
    pub fn legacy_sessions(&self) -> Option<u32> {
        self.offer.legacy_sessions()
    }
    pub(crate) fn new(
        request_id: Uuid,
        target: HostActionTarget,
        offer: Arc<Charged<HostUpdateOffer>>,
        backend: Option<&str>,
        budget: &ConnectionBudget,
    ) -> io::Result<Self> {
        let replacement = offer
            .to_version()
            .or(backend)
            .filter(|value| valid_version(value))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Backend version is unavailable; refresh it before updating",
                )
            })?;
        let allocation = budget
            .reserve(
                std::mem::size_of::<Self>() + target.target().len() + replacement.len(),
                false,
            )
            .map_err(io::Error::other)?;
        Ok(Self {
            request_id,
            target,
            replacement: replacement.into(),
            offer,
            _allocation: allocation,
        })
    }
    pub(crate) fn matches(
        &self,
        action: &HostActionTarget,
        offer: &HostUpdateOffer,
        backend: Option<&str>,
    ) -> bool {
        &self.target == action
            && self.offer.value == *offer
            && offer.to_version().or(backend) == Some(self.to_version())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostUpdateState {
    #[default]
    Idle,
    Closing,
    Updating,
    /// Dispatch happened but fresh application readiness was not confirmed.
    Unconfirmed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn captured_plan_requires_current_identity_and_exact_bounded_offer_facts() {
        use crate::workspace::WindowWorkspace;
        let mut window = WindowWorkspace::new(Uuid::new_v4());
        let (key, _) = window
            .ui_test_remote_terminal_fixture("update.test")
            .unwrap();
        let action = window
            .host(key.source_id())
            .unwrap()
            .action_target()
            .clone();
        let budget = ConnectionBudget::default();
        let offer = HostUpdateOffer {
            from_version: "0.1.0".into(),
            to_version: Some("0.2.0".into()),
            legacy_sessions: Some(3),
        };
        let charged = |offer: HostUpdateOffer| {
            Arc::new(
                budget
                    .reserve(offer.retained_bytes(), false)
                    .unwrap()
                    .charge(offer),
            )
        };
        window.host_mut(&action).unwrap().update_offer = Some(charged(offer.clone()));
        let plan = HostUpdatePlan::new(
            Uuid::new_v4(),
            action.clone(),
            charged(offer.clone()),
            None,
            &budget,
        )
        .unwrap();
        assert!(
            window.accepts_host_update_plan(&plan),
            "legacy uses versions in its own offer"
        );
        for changed in [
            HostUpdateOffer {
                from_version: "0.1.1".into(),
                ..offer.clone()
            },
            HostUpdateOffer {
                to_version: Some("0.2.1".into()),
                ..offer.clone()
            },
            HostUpdateOffer {
                legacy_sessions: Some(4),
                ..offer.clone()
            },
            HostUpdateOffer {
                to_version: None,
                legacy_sessions: None,
                ..offer.clone()
            },
        ] {
            window.host_mut(&action).unwrap().update_offer = Some(charged(changed));
            assert!(!window.accepts_host_update_plan(&plan));
        }
        window.host_mut(&action).unwrap().update_offer = None;
        assert!(!window.accepts_host_update_plan(&plan));
        window.host_mut(&action).unwrap().update_offer = Some(charged(offer));
        window.disconnect(key.source_id()).unwrap();
        window.finish_attempt(key.source_id()).unwrap();
        window.retry(key.source_id()).unwrap();
        assert!(
            !window.accepts_host_update_plan(&plan),
            "attempt replacement revokes capture"
        );
        drop(plan);
        drop(window);
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn ready_daemon_version_is_bounded_display_text_without_an_update() {
        let budget = ConnectionBudget::default();
        let version = daemon_version_from_ready(
            &json!({"daemon":{"version":"0.3.2","protocol":1,"started":false}}),
            &budget,
        )
        .unwrap();
        assert_eq!(version.value, "0.3.2");
        assert!(budget.used() > 0);
        drop(version);
        assert_eq!(budget.used(), 0);
        for value in [
            json!({}),
            json!({"daemon":{}}),
            json!({"daemon":{"version":7}}),
            json!({"daemon":{"version":""}}),
            json!({"daemon":{"version":"x".repeat(65)}}),
            json!({"daemon":{"version":"0.3.2\u{1b}[31m"}}),
        ] {
            assert!(daemon_version_from_ready(&value, &budget).is_none());
            assert_eq!(budget.used(), 0);
        }
    }
    #[test]
    fn a_host_names_its_daemon_version_only_while_connected_in_this_attempt() {
        use crate::workspace::WindowWorkspace;
        let mut window = WindowWorkspace::new(Uuid::new_v4());
        let (key, _) = window
            .ui_test_remote_terminal_fixture("version.test")
            .unwrap();
        let id = key.source_id();
        assert_eq!(window.host(id).unwrap().daemon_version(), None);
        let action = window.host(id).unwrap().action_target().clone();
        window
            .ui_test_host_daemon_version(&action, &json!({"daemon":{"version":"0.3.2"}}))
            .unwrap();
        assert_eq!(window.host(id).unwrap().daemon_version(), Some("0.3.2"));
        window.disconnect(id).unwrap();
        assert_eq!(window.host(id).unwrap().daemon_version(), None);
        window.finish_attempt(id).unwrap();
        window.retry(id).unwrap();
        assert!(
            window.host(id).unwrap().daemon_version.is_none(),
            "a new attempt forgets the earlier daemon's version"
        );
    }
    #[test]
    fn ready_offers_are_bounded_keeper_facts_not_version_policy() {
        let budget = ConnectionBudget::default();
        let ready = json!({"daemon":{"version":"not-a-semver-policy-input","keepsSessions":true},"update":{"available":true,"keepsSessions":true}});
        let offer = HostUpdateOffer::from_ready(&ready, &budget)
            .unwrap()
            .unwrap();
        assert_eq!(offer.from_version(), "not-a-semver-policy-input");
        assert!(offer.to_version().is_none());
        assert!(budget.used() > 0);
        drop(offer);
        assert_eq!(budget.used(), 0);
        for value in [
            json!({"daemon":{"version":"0.1.0","keepsSessions":false},"update":{"available":true,"keepsSessions":true}}),
            json!({"daemon":{"version":"x".repeat(65),"keepsSessions":true},"update":{"available":true,"keepsSessions":true}}),
            json!({"daemon":{"version":"0.1.0","keepsSessions":true},"update":{"available":true,"keepsSessions":false}}),
            json!({"update":{"available":"true","keepsSessions":true}}),
        ] {
            assert!(HostUpdateOffer::from_ready(&value, &budget).is_err());
            assert_eq!(budget.used(), 0);
        }
        assert!(HostUpdateOffer::from_ready(&json!({}), &budget)
            .unwrap()
            .is_none());
        assert!(HostUpdateOffer::from_ready(
            &json!({"update":{"available":false,"keepsSessions":true}}),
            &budget
        )
        .unwrap()
        .is_none());
    }
}
