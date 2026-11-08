//! Independent, typed notification switches from the assigned local service.
use crate::api_stream::ApiStream;
use serde::Deserialize;
use splitty_core::{config::NotificationSettings, native_api::Operation};
use std::io;
use uuid::Uuid;

/// One completed read. The window decides whether its owner and generation are current.
#[derive(Debug)]
pub struct NotificationSettingsRead {
    connection_id: Uuid,
    generation: u64,
    result: Result<NotificationSettings, String>,
}

impl NotificationSettingsRead {
    #[cfg(feature = "ui-test-support")]
    #[doc(hidden)]
    pub fn ui_test_fixture(
        connection_id: Uuid,
        generation: u64,
        result: Result<NotificationSettings, String>,
    ) -> Self {
        Self {
            connection_id,
            generation,
            result,
        }
    }

    pub(crate) fn new(
        connection_id: Uuid,
        generation: u64,
        result: io::Result<NotificationSettings>,
    ) -> Self {
        Self {
            connection_id,
            generation,
            result: result.map_err(|error| error.to_string().chars().take(256).collect()),
        }
    }

    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn result(&self) -> Result<&NotificationSettings, &str> {
        self.result.as_ref().map_err(String::as_str)
    }
}

pub(crate) async fn read(client: &mut ApiStream) -> io::Result<NotificationSettings> {
    #[derive(Deserialize)]
    struct HomeNotifications {
        notifications: NotificationSettings,
    }

    let reply = client
        .request(Operation::HomeGet, serde_json::Map::new())
        .await?;
    if reply.uncertain {
        return Err(io::Error::other(
            "local notification settings read outcome is unknown",
        ));
    }
    let home = crate::local_service::decode::<HomeNotifications>(client, &reply)?;
    // Six booleans are owned after dropping the charged reply and typed decode.
    Ok(home.notifications.clone())
}
