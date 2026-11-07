use super::decode;
use crate::api_stream::ApiStream as Client;
use crate::appearance::{DrawerVisibilityOutcome, DrawerVisibilityResult};
use serde::Deserialize;
use splitty_core::native_api::Operation;
use splitty_transport::wire;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Deserialize)]
struct Home {
    drawer: Drawer,
}

#[derive(Deserialize)]
struct Drawer {
    pinned: bool,
}

pub(super) async fn execute(
    client: &mut Client,
    owner: Uuid,
    request: Uuid,
    pinned: bool,
    cancel: &Notify,
) -> DrawerVisibilityResult {
    let args = serde_json::json!({"body": {"drawer": {"pinned": pinned}}})
        .as_object()
        .expect("object literal")
        .clone();
    let reply = tokio::select! { biased;
        _ = cancel.notified() => uncertain("drawer visibility update cancelled; outcome is unknown"),
        result = client.request(Operation::HomeUpdate, args) => match result {
            Ok(reply) => classify(client, &reply),
            // The API stream does not distinguish failure before and after admission.
            Err(_) => uncertain("drawer visibility update reply unavailable; outcome is unknown"),
        },
    };
    DrawerVisibilityResult {
        connection_id: owner,
        request_id: request,
        outcome: reply,
    }
}

fn classify(client: &Client, reply: &wire::Reply) -> DrawerVisibilityOutcome {
    if reply.uncertain {
        return uncertain("drawer visibility update outcome is unknown; refreshing settings");
    }
    if !(200..300).contains(&reply.status) {
        return DrawerVisibilityOutcome::Rejected {
            reason: "service rejected drawer visibility update".into(),
        };
    }
    match decode::<Home>(client, reply) {
        Ok(home) => DrawerVisibilityOutcome::Confirmed(home.drawer.pinned),
        Err(_) => uncertain("drawer visibility update reply was invalid; refreshing settings"),
    }
}

fn uncertain(reason: &str) -> DrawerVisibilityOutcome {
    DrawerVisibilityOutcome::Uncertain {
        reason: reason.into(),
    }
}
