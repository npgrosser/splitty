//! Commands that need a capability of their Screen's server.
//!
//! A window offers an action whatever the server it would reach; the command
//! layer looks the command's capability up here before anything is sent. A
//! server whose `mux.welcome` did not list it gets nothing, and the window
//! gets one status message that the server needs an update. Only
//! capabilities decide, never versions. A command that needs a newer server
//! is one more row of [`required_capability`].
//!
//! Older capability checks that change what a command does rather than
//! whether it is sent (a close without `close-assessment-v1` asks
//! generically, a tab drop without `tab-placement-v1` is not offered) stay
//! where they are.
use super::{rejected_command, Command, CommandKind, Update};
use splitty_core::protocol::capability;

/// The `mux.welcome` capability `kind` needs, if any.
pub(super) fn required_capability(kind: &CommandKind) -> Option<&'static str> {
    match kind {
        CommandKind::MoveGroup { .. } => Some(capability::LAYOUT_ARRANGE),
        _ => None,
    }
}

/// What the window shows for a command its server cannot take; `remote` for
/// a host's daemon, otherwise the window's local service.
pub(super) fn outdated_server(remote: bool) -> String {
    if remote {
        "The host's Splitty daemon is older than this app; update it for this action".into()
    } else {
        "The local Splitty service is older than this app; update it for this action".into()
    }
}

/// Whether `offered`, a server's announced capabilities, lets `kind` through.
pub(super) fn admits(kind: &CommandKind, offered: &[String]) -> bool {
    required_capability(kind)
        .is_none_or(|needed| offered.iter().any(|capability| capability == needed))
}

/// What the window gets instead of a command its server cannot take, or
/// none when the command may go: a pending action of the window settles
/// first through the command's own rejection, then the one status
/// message is the last word.
pub(super) fn refusal(command: &Command, offered: &[String], remote: bool) -> Option<Vec<Update>> {
    if admits(&command.kind, offered) {
        return None;
    }
    let reason = outdated_server(remote);
    let mut updates = Vec::new();
    let settled = rejected_command(command.source, &command.kind, reason.clone());
    if !matches!(settled, Update::Status(_)) {
        updates.push(settled);
    }
    updates.push(Update::ServerOutdated(reason));
    Some(updates)
}

#[cfg(test)]
mod tests;
