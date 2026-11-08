//! Identified local metadata intents; connection continuation stays in the worker.
use super::*;
use crate::workspace::SourceToken;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostManagementAction {
    Connect,
    SetAlias,
    Forget,
}
#[derive(Clone, Debug)]
pub struct HostManagementRequest {
    target: String,
    alias: Option<String>,
    action: HostManagementAction,
}
impl HostManagementRequest {
    pub fn connect(target: &str, alias: Option<&str>) -> io::Result<Self> {
        Self::new(target, alias, HostManagementAction::Connect)
    }
    pub fn set_alias(target: &str, alias: &str) -> io::Result<Self> {
        Self::new(target, Some(alias), HostManagementAction::SetAlias)
    }
    pub fn forget(target: &str) -> io::Result<Self> {
        Self::new(target, None, HostManagementAction::Forget)
    }
    fn new(target: &str, alias: Option<&str>, action: HostManagementAction) -> io::Result<Self> {
        let target = SshTarget::parse(target).map_err(io::Error::other)?;
        if let Some(alias) = alias {
            if alias.chars().any(char::is_control) || alias.trim().chars().count() > 200 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Use at most 200 characters without controls for the display name",
                ));
            }
        }
        Ok(Self {
            target: target.as_str().into(),
            alias: alias
                .map(|s| s.trim().to_owned())
                .filter(|alias| action != HostManagementAction::Connect || !alias.is_empty()),
            action,
        })
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn action(&self) -> HostManagementAction {
        self.action
    }
    fn body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({"target":self.target});
        if self.action == HostManagementAction::Connect {
            body["hidden"] = false.into();
        }
        if let Some(alias) = &self.alias {
            body["alias"] = if alias.is_empty() {
                serde_json::Value::Null
            } else {
                alias.clone().into()
            };
        }
        body
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostManagementOutcome {
    Pending,
    Applied,
    Rejected,
    Uncertain,
}
#[derive(Clone, Debug)]
pub struct HostManagementResult {
    owner: SourceToken,
    request_id: Uuid,
    target: String,
    action: HostManagementAction,
    outcome: HostManagementOutcome,
    reason: Option<String>,
}
impl HostManagementResult {
    #[cfg(feature = "ui-test-support")]
    #[doc(hidden)]
    pub fn ui_test_fixture(
        owner: SourceToken,
        request_id: Uuid,
        target: String,
        action: HostManagementAction,
        outcome: HostManagementOutcome,
        reason: Option<String>,
    ) -> Self {
        Self {
            owner,
            request_id,
            target,
            action,
            outcome,
            reason,
        }
    }
    pub fn owner(&self) -> SourceToken {
        self.owner
    }
    pub fn request_id(&self) -> Uuid {
        self.request_id
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn action(&self) -> HostManagementAction {
        self.action
    }
    pub fn outcome(&self) -> HostManagementOutcome {
        self.outcome
    }
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
    pub(crate) fn connection_failed(&mut self, reason: String) {
        self.outcome = HostManagementOutcome::Rejected;
        self.reason = Some(format!(
            "Host metadata was saved, but the connection could not start: {}",
            reason.chars().take(384).collect::<String>()
        ));
    }
    pub(crate) fn new(
        owner: SourceToken,
        request_id: Uuid,
        request: &HostManagementRequest,
        outcome: HostManagementOutcome,
        reason: Option<String>,
    ) -> Self {
        Self {
            owner,
            request_id,
            target: request.target.clone(),
            action: request.action,
            outcome,
            reason: reason.map(|s| s.chars().take(512).collect()),
        }
    }
}
pub(super) struct Job {
    pub owner: SourceToken,
    pub id: Uuid,
    pub request: HostManagementRequest,
}
impl Job {
    pub fn result(
        &self,
        outcome: HostManagementOutcome,
        reason: Option<String>,
    ) -> HostManagementResult {
        HostManagementResult::new(self.owner, self.id, &self.request, outcome, reason)
    }
}
pub(super) async fn write(client: &mut ApiStream, job: &Job) -> (HostManagementResult, bool) {
    use HostManagementOutcome::*;
    let op = if job.request.action == HostManagementAction::Forget {
        Operation::HostForget
    } else {
        Operation::HostPreferencesUpdate
    };
    let args = serde_json::Map::from_iter([("body".into(), job.request.body())]);
    let reply = match client.request(op, args).await {
        Ok(reply) => reply,
        Err(_) => {
            return (
                job.result(
                    Uncertain,
                    Some("The host metadata reply was lost. Refresh before trying again.".into()),
                ),
                true,
            )
        }
    };
    if reply.uncertain {
        return (
            job.result(
                Uncertain,
                Some("The host metadata outcome is uncertain. Refresh before trying again.".into()),
            ),
            false,
        );
    }
    if !(200..300).contains(&reply.status) {
        // A failed durable write can still have partially changed host metadata.
        let outcome = if reply.status >= 500 {
            Uncertain
        } else {
            Rejected
        };
        return (
            job.result(
                outcome,
                Some(format!(
                    "Could not change host metadata (status {}).",
                    reply.status
                )),
            ),
            false,
        );
    }
    let valid = if op == Operation::HostForget {
        reply.status == 204 && reply.body.is_empty()
    } else {
        decode::<splitty_remote::hosts::HostPreference>(client, &reply).is_ok_and(|row| {
            row.target == job.request.target
                && (job.request.action != HostManagementAction::Connect || !row.hidden)
                && job.request.alias.as_ref().is_none_or(|alias| {
                    row.alias.as_deref()
                        == if alias.is_empty() {
                            None
                        } else {
                            Some(alias.as_str())
                        }
                })
        })
    };
    if valid {
        (job.result(Applied, None), false)
    } else {
        (
            job.result(
                Uncertain,
                Some("The service returned an invalid host metadata acknowledgement.".into()),
            ),
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_validation_preserves_clear_versus_omitted_alias() {
        for target in ["", "-option", "host;touch /tmp/bad", "user@host\ncommand"] {
            assert!(HostManagementRequest::connect(target, None).is_err());
        }
        for alias in ["bad\nname".to_string(), "x".repeat(201)] {
            assert!(HostManagementRequest::set_alias("vm1", &alias).is_err());
        }
        let request = HostManagementRequest::connect(" vm1 ", None).unwrap();
        assert_eq!(request.target(), "vm1");
        assert!(request.body().get("alias").is_none());
        assert_eq!(request.body()["hidden"], false);
        assert!(HostManagementRequest::connect("vm1", Some("  "))
            .unwrap()
            .body()
            .get("alias")
            .is_none());
        assert!(HostManagementRequest::connect("vm1", Some("\n")).is_err());
        assert!(HostManagementRequest::set_alias("vm1", "  ")
            .unwrap()
            .body()["alias"]
            .is_null());
        assert_eq!(
            HostManagementRequest::set_alias("vm1", " Build ")
                .unwrap()
                .body()["alias"],
            "Build"
        );
    }
}
