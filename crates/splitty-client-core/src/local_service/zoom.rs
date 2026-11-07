use super::decode;
use crate::zoom::{Zoom, ZoomAction, ZoomOutcome, ZoomResult};
use serde::Deserialize;
use splitty_server::{native::client::Client, service::catalog::Operation};
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
    fn zoom(&self) -> Zoom {
        Zoom::from_values(self.appearance.zoom, &self.appearance.zoom_scope)
    }
}

pub(super) async fn execute(
    client: &mut Client,
    owner: Uuid,
    id: Uuid,
    action: ZoomAction,
    cancel: &Notify,
) -> ZoomResult {
    let outcome = execute_inner(client, action, cancel).await;
    ZoomResult {
        connection_id: owner,
        request_id: id,
        outcome,
    }
}
async fn execute_inner(client: &mut Client, action: ZoomAction, cancel: &Notify) -> ZoomOutcome {
    let read = tokio::select! { biased;
        _ = cancel.notified() => return rejected("zoom cancelled before update"),
        result = client.request(Operation::HomeGet, serde_json::Map::new()) => result,
    };
    let current = match read.and_then(|reply| {
        if reply.uncertain {
            return Err(std::io::Error::other("uncertain settings read"));
        }
        decode::<Home>(client, &reply)
    }) {
        Ok(home) => home.zoom(),
        Err(_) => return rejected("could not read current zoom; no update submitted"),
    };
    let next = current.apply(action);
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
        Ok(reply) => classify(client, &reply),
        // The transport does not expose an admission-stage token. Conservatively
        // report uncertainty for every request error, never retry the mutation.
        Err(_) => uncertain("zoom update reply unavailable; outcome is unknown"),
    }
}
fn classify(client: &Client, reply: &wire::Reply) -> ZoomOutcome {
    if reply.uncertain {
        return uncertain("zoom update outcome is unknown; refreshing settings");
    }
    if !(200..300).contains(&reply.status) {
        return rejected("service rejected zoom update");
    }
    match decode::<Home>(client, reply) {
        Ok(home)
            if home.appearance.zoom.is_finite() && (0.5..=3.0).contains(&home.appearance.zoom) =>
        {
            ZoomOutcome::Confirmed(home.zoom())
        }
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
