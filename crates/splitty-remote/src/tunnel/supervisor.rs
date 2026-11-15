//! Host version policy and bounded retry delays for forwarding workers.
use super::MigrationOffer;
use crate::handshake::{LegacyMigration, RunningDaemon};
pub use crate::ssh::PREPARE_TIMEOUT;
use std::time::Duration;

/// What the hub does about a daemon of another version than the client's.
#[derive(Debug, PartialEq, Eq)]
pub enum UpdatePlan {
    /// Stop it and start this client's: it is older, its sessions outlive it,
    /// and the host's answering server is this client's version.
    Update,
    /// Use it as it is, and tell the user why.
    Notice(String),
}

pub fn update_plan(
    daemon: &RunningDaemon,
    host: &str,
    version: &str,
    server: Option<&str>,
) -> UpdatePlan {
    let host_version = &daemon.version;
    if !older(host_version, version) {
        // Never replace a newer daemon: it could fall back to a version
        // without keepers, which would start fresh shells and leave the
        // kept ones unadopted.
        return UpdatePlan::Notice(format!(
            "The daemon on {host} runs Splitty {host_version}, this client is {version}; \
             it is used as it is."
        ));
    }
    if !daemon.keeps_sessions {
        return UpdatePlan::Notice(format!(
            "The daemon on {host} runs Splitty {host_version}, this client is {version}. \
             Updating it ends its sessions: to update, stop it there (`daemon stop`) and \
             connect again."
        ));
    }
    if daemon.instance.is_none() {
        return UpdatePlan::Notice(format!(
            "The legacy daemon on {host} has no verifiable service instance. Explicitly restart it on the host before connecting; its kept sessions remain available."
        ));
    }
    if server != Some(version) {
        return UpdatePlan::Notice(format!(
            "The daemon on {host} runs Splitty {host_version}, this client is {version}. \
             `splitty daemon restart` there, with a newer splitty, updates it without ending \
             its sessions."
        ));
    }
    UpdatePlan::Update
}

/// Only a positively verified legacy offer and the exact replacement version
/// can authorize the explicit action. Callers retain/account the cloned fields.
pub fn migration_offer(
    daemon: &RunningDaemon,
    version: &str,
    server: Option<&str>,
) -> Option<MigrationOffer> {
    let LegacyMigration::Ready { token, sessions } = daemon.legacy_migration.as_ref()? else {
        return None;
    };
    if !daemon.legacy
        || daemon.started
        || !daemon.keeps_sessions
        || daemon.instance.is_some()
        || daemon.version.len() > 64
        || version.len() > 64
        || !older(&daemon.version, version)
        || server != Some(version)
        || token.len() != 64
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    Some(MigrationOffer {
        available: true,
        from_version: daemon.version.clone(),
        to_version: version.into(),
        sessions: *sessions,
    })
}

/// Compare SemVer precedence, including prereleases but excluding build
/// metadata. One optional `v` prefix is accepted; invalid versions never
/// authorize replacing a daemon.
pub fn older(a: &str, b: &str) -> bool {
    fn parse(version: &str) -> Result<semver::Version, semver::Error> {
        semver::Version::parse(version.strip_prefix('v').unwrap_or(version))
    }
    match (parse(a), parse(b)) {
        (Ok(a), Ok(b)) => a.cmp_precedence(&b).is_lt(),
        _ => false,
    }
}

pub fn backoff(failures: u32) -> Duration {
    Duration::from_secs(1u64.checked_shl(failures).unwrap_or(u64::MAX).min(10))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_precedence_ignores_build_metadata_and_orders_prereleases() {
        for (older_version, newer_version) in [
            ("0.2.0-dev.9", "0.2.0-dev.10"),
            ("0.2.0-dev.1", "0.2.0"),
            ("v0.2.0", "0.3.0"),
        ] {
            assert!(older(older_version, newer_version));
            assert!(!older(newer_version, older_version));
        }
        for (a, b) in [
            ("0.2.0+first", "0.2.0+second"),
            ("invalid", "0.3.0"),
            ("0.2.0", "invalid"),
            ("0.2.0", "0.2.0"),
        ] {
            assert!(!older(a, b));
            assert!(!older(b, a));
        }
    }
    #[test]
    fn backoff_is_bounded() {
        assert_eq!(
            (0..6).map(backoff).collect::<Vec<_>>(),
            [1, 2, 4, 8, 10, 10].map(Duration::from_secs)
        );
        assert_eq!(backoff(u32::MAX), Duration::from_secs(10));
    }
}
