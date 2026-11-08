//! Independent, typed notification switches from the assigned local service.
use crate::{
    api_stream::ApiStream,
    settings::{SettingsField, SettingsMetadata, SettingsValue},
};
use serde::Deserialize;
use splitty_core::{config::NotificationSettings, native_api::Operation};
use std::{collections::BTreeMap, io};
use uuid::Uuid;

#[derive(Deserialize)]
pub(super) struct HomeNotifications {
    pub(super) notifications: NotificationSettings,
}

/// Only bounded values escape the charged settings/metadata replies.
#[derive(Clone, Debug)]
pub(super) struct NotificationSnapshot {
    values: NotificationSettings,
    defaults: BTreeMap<SettingsField, bool>,
    metadata_error: Option<String>,
}

fn value(settings: &NotificationSettings, field: SettingsField) -> Option<SettingsValue> {
    Some(SettingsValue::Bool(match field {
        SettingsField::NotificationEnabled => settings.enabled,
        SettingsField::NotificationProcessFinished => settings.process_finished,
        SettingsField::NotificationProcessExited => settings.process_exited,
        SettingsField::NotificationProcessCrashed => settings.process_crashed,
        _ => return None,
    }))
}

/// One completed read. The window decides whether its owner and generation are current.
#[derive(Clone, Debug)]
pub struct NotificationSettingsRead {
    connection_id: Uuid,
    generation: u64,
    result: Result<NotificationSnapshot, String>,
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
            result: result.map(|values| NotificationSnapshot {
                values,
                defaults: BTreeMap::new(),
                metadata_error: None,
            }),
        }
    }

    #[cfg(feature = "ui-test-support")]
    #[doc(hidden)]
    pub fn ui_test_with_defaults(
        connection_id: Uuid,
        generation: u64,
        settings: NotificationSettings,
        defaults: NotificationSettings,
    ) -> Self {
        let defaults = SettingsField::ALL
            .into_iter()
            .filter_map(|field| match value(&defaults, field) {
                Some(SettingsValue::Bool(default)) => Some((field, default)),
                _ => None,
            })
            .collect();
        Self {
            connection_id,
            generation,
            result: Ok(NotificationSnapshot {
                values: settings,
                defaults,
                metadata_error: None,
            }),
        }
    }

    pub(super) fn new(
        connection_id: Uuid,
        generation: u64,
        result: io::Result<NotificationSnapshot>,
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
    /// Effective values remain available when reset metadata fails.
    pub fn result(&self) -> Result<&NotificationSettings, &str> {
        self.result
            .as_ref()
            .map(|snapshot| &snapshot.values)
            .map_err(String::as_str)
    }
    pub fn value(&self, field: SettingsField) -> Option<SettingsValue> {
        value(self.result().ok()?, field)
    }
    pub fn default_value(&self, field: SettingsField) -> Option<SettingsValue> {
        self.result
            .as_ref()
            .ok()?
            .defaults
            .get(&field)
            .copied()
            .map(SettingsValue::Bool)
    }
    pub fn metadata_error(&self) -> Option<&str> {
        self.result.as_ref().ok()?.metadata_error.as_deref()
    }
}

pub(super) async fn read(client: &mut ApiStream) -> io::Result<NotificationSnapshot> {
    let values = {
        let reply = client
            .request(Operation::HomeGet, serde_json::Map::new())
            .await?;
        if reply.uncertain {
            return Err(io::Error::other(
                "local notification settings read outcome is unknown",
            ));
        }
        let home = crate::local_service::decode::<HomeNotifications>(client, &reply)?;
        // Six required booleans prove compatible edit choices, without metadata.
        home.notifications.clone()
    };
    let metadata = async {
        let reply = client
            .request(Operation::HomeMetadata, serde_json::Map::new())
            .await?;
        if reply.uncertain {
            return Err(io::Error::other(
                "notification reset defaults read outcome is unknown",
            ));
        }
        let metadata = crate::local_service::decode::<SettingsMetadata>(client, &reply)?;
        metadata.validate()?;
        let defaults = SettingsField::ALL
            .into_iter()
            .filter_map(|field| {
                if field.domain() != crate::settings::SettingsDomain::Notifications {
                    return None;
                }
                match metadata.default_value(field) {
                    Some(SettingsValue::Bool(default)) => Some((field, default)),
                    _ => None,
                }
            })
            .collect::<BTreeMap<_, _>>();
        Ok(defaults)
    }
    .await;
    let (defaults, metadata_error) = match metadata {
        Ok(defaults) => {
            let error = (defaults.len() != 4)
                .then(|| "Some notification reset defaults are unavailable".to_owned());
            (defaults, error)
        }
        Err(error) => (
            BTreeMap::new(),
            Some(error.to_string().chars().take(256).collect()),
        ),
    };
    Ok(NotificationSnapshot {
        values,
        defaults,
        metadata_error,
    })
}
