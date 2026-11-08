use super::{theme_name, AppearanceSnapshot, ColorScheme, HomePresentation, TerminalPreferences};
use crate::api_stream::ApiStream as Client;
use crate::local_service::decode;
use crate::settings::{SettingsDomain, SettingsMetadata};
use serde::Deserialize;
use splitty_core::native_api::Operation;
use splitty_core::{
    config::{AppearanceSettings, DrawerSettings},
    theme::ResolvedTheme,
};
use splitty_transport::budget::Charged;
use std::{io, sync::Arc};
use uuid::Uuid;

#[derive(Deserialize)]
pub(crate) struct HomeAppearance {
    pub appearance: AppearanceSettings,
    pub drawer: DrawerSettings,
    pub terminal: TerminalPreferences,
}
impl HomeAppearance {
    pub(crate) fn validate(&self) -> io::Result<()> {
        if [
            &self.appearance.theme,
            &self.appearance.theme_dark,
            &self.appearance.theme_light,
            &self.appearance.theme_mode,
            &self.appearance.zoom_scope,
            self.terminal.theme(),
            self.terminal.close_on_exit(),
        ]
        .iter()
        .any(|s| s.len() > 64)
            || self.appearance.font_family.len() > 4096
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "appearance string exceeds native limit",
            ));
        }
        Ok(())
    }
}

async fn read_typed<T: serde::de::DeserializeOwned>(
    client: &mut Client,
    operation: Operation,
) -> io::Result<Charged<T>> {
    let reply = client.request(operation, serde_json::Map::new()).await?;
    if reply.uncertain {
        return Err(io::Error::other("local settings read outcome is unknown"));
    }
    decode(client, &reply)
}
async fn read_theme(client: &mut Client, name: &str) -> io::Result<Arc<Charged<ResolvedTheme>>> {
    let mut args = serde_json::Map::new();
    args.insert("name".into(), name.into());
    let reply = client.request(Operation::ThemeGet, args).await?;
    if reply.uncertain {
        return Err(io::Error::other("local theme read outcome is unknown"));
    }
    let theme = decode::<ResolvedTheme>(client, &reply)?;
    Ok(Arc::new(theme))
}

pub(crate) async fn read(
    client: &mut Client,
    connection_id: Uuid,
    generation: u64,
    scheme: ColorScheme,
    terminal_generation: u64,
    previous: Option<Arc<AppearanceSnapshot>>,
    observed: &mut Option<SettingsDomain>,
) -> io::Result<AppearanceSnapshot> {
    let home = read_typed::<HomeAppearance>(client, Operation::HomeGet).await?;
    home.validate()?;
    *observed = Some(SettingsDomain::Appearance);
    let settings = home.map(|home| HomePresentation {
        appearance: home.appearance,
        drawer: home.drawer,
        terminal: home.terminal,
    });
    let theme = read_theme(client, theme_name(&settings.appearance, scheme)).await?;
    let metadata = read_typed::<SettingsMetadata>(client, Operation::HomeMetadata)
        .await
        .and_then(|metadata| {
            metadata.validate()?;
            Ok(metadata)
        });
    let themes = read_typed::<Vec<String>>(client, Operation::ThemeList)
        .await
        .and_then(|names| {
            if names.len() > 1024 || names.iter().any(|name| name.is_empty() || name.len() > 64) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Theme catalog exceeds native limit",
                ));
            }
            Ok(names)
        });
    let (terminal_theme, terminal_palette_error) = if settings.terminal.theme() == "default" {
        (None, None)
    } else {
        let palette = if themes
            .as_ref()
            .is_ok_and(|names| names.iter().any(|name| name == settings.terminal.theme()))
        {
            read_theme(client, settings.terminal.theme()).await.and_then(|theme| {
                if theme.id.as_deref() == Some(settings.terminal.theme()) { Ok(theme) }
                else { Err(io::Error::other("Terminal palette identity is unavailable or the service returned a fallback")) }
            })
        } else {
            Err(io::Error::other(
                "Terminal palette is unavailable in the local theme catalog",
            ))
        };
        match palette {
            Ok(theme) => (Some(theme), None),
            Err(error) => (
                previous
                    .as_ref()
                    .map(|p| p.terminal_theme.clone().unwrap_or_else(|| p.theme.clone())),
                Some(error.to_string()),
            ),
        }
    };
    let (metadata, metadata_error) = match metadata {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let (themes, themes_error) = match themes {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(error.to_string())),
    };
    Ok(AppearanceSnapshot {
        connection_id,
        generation,
        settings,
        theme,
        terminal_theme,
        terminal_generation,
        terminal_palette_error,
        metadata,
        themes,
        metadata_error,
        themes_error,
    })
}

#[cfg(all(test, not(target_os = "windows")))]
pub(crate) mod tests;
