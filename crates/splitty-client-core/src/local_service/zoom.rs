use super::decode;
use crate::api_stream::ApiStream as Client;
use crate::settings::SettingsMetadata;
use crate::zoom::{Zoom, ZoomAction, ZoomOutcome, ZoomResult, ZoomSteps};
use serde::Deserialize;
use splitty_core::native_api::Operation;
use splitty_transport::wire;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Deserialize)]
struct Home {
    appearance: Appearance,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Appearance {
    zoom: f64,
    zoom_scope: String,
}
impl Home {
    fn zoom(&self, steps: &ZoomSteps) -> Zoom {
        Zoom::from_values(self.appearance.zoom, &self.appearance.zoom_scope, steps)
    }
}

/// Carry out the actions that were waiting as one read of the settings
/// metadata, one of the settings and at most one write: the level is the
/// current one with every action applied in order, in the service's levels.
/// The outcome is that of them all; the result names the first.
pub(super) async fn execute(
    client: &mut Client,
    owner: Uuid,
    actions: &[(Uuid, ZoomAction)],
    cancel: &Notify,
) -> ZoomResult {
    let outcome = execute_inner(client, actions, cancel).await;
    ZoomResult {
        connection_id: owner,
        request_id: actions.first().map(|(id, _)| *id).unwrap_or_default(),
        outcome,
    }
}
async fn execute_inner(
    client: &mut Client,
    actions: &[(Uuid, ZoomAction)],
    cancel: &Notify,
) -> ZoomOutcome {
    let read = tokio::select! { biased;
        _ = cancel.notified() => return rejected("zoom cancelled before update"),
        result = client.request(Operation::HomeMetadata, serde_json::Map::new()) => result,
    };
    // A service without metadata answers with an error status: its levels
    // are those of a service without metadata.
    let steps = match read {
        Ok(reply) if !reply.uncertain => {
            let metadata = (200..300)
                .contains(&reply.status)
                .then(|| decode::<SettingsMetadata>(client, &reply).ok())
                .flatten()
                .filter(|metadata| metadata.validate().is_ok());
            ZoomSteps::from_metadata(metadata.as_deref())
        }
        _ => return rejected("could not read the zoom levels; no update submitted"),
    };
    let read = tokio::select! { biased;
        _ = cancel.notified() => return rejected("zoom cancelled before update"),
        result = client.request(Operation::HomeGet, serde_json::Map::new()) => result,
    };
    let stored = match read.and_then(|reply| {
        if reply.uncertain {
            return Err(std::io::Error::other("uncertain settings read"));
        }
        decode::<Home>(client, &reply)
    }) {
        Ok(home) => home,
        Err(_) => return rejected("could not read current zoom; no update submitted"),
    };
    let current = stored.zoom(&steps);
    let next = actions
        .iter()
        .fold(current, |zoom, (_, action)| zoom.apply(*action, &steps));
    if next == current {
        return ZoomOutcome::Confirmed(current);
    }
    let args = serde_json::json!({"body": {"appearance": {"zoom": next.level()}}})
        .as_object()
        .expect("object literal")
        .clone();
    let reply = tokio::select! { biased;
        _ = cancel.notified() => return uncertain("zoom update cancelled; outcome is unknown"),
        result = client.request(Operation::HomeUpdate, args) => result,
    };
    match reply {
        Ok(reply) => classify(client, &reply, &steps),
        // The transport does not expose an admission-stage token. Conservatively
        // report uncertainty for every request error, never retry the mutation.
        Err(_) => uncertain("zoom update reply unavailable; outcome is unknown"),
    }
}
fn classify(client: &Client, reply: &wire::Reply, steps: &ZoomSteps) -> ZoomOutcome {
    if reply.uncertain {
        return uncertain("zoom update outcome is unknown; refreshing settings");
    }
    if !(200..300).contains(&reply.status) {
        return rejected("service rejected zoom update");
    }
    match decode::<Home>(client, reply) {
        Ok(home) if steps.admits(home.appearance.zoom) => ZoomOutcome::Confirmed(home.zoom(steps)),
        _ => uncertain("zoom update reply was invalid; refreshing settings"),
    }
}
fn rejected(reason: &str) -> ZoomOutcome {
    ZoomOutcome::Rejected {
        reason: reason.into(),
    }
}
fn uncertain(reason: &str) -> ZoomOutcome {
    ZoomOutcome::Uncertain {
        reason: reason.into(),
    }
}
