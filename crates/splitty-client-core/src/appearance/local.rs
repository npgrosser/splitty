use super::{AppearanceSnapshot, ColorScheme, HomePresentation, TerminalPreferences};
use crate::api_stream::ApiStream as Client;
use crate::local_service::decode;
use crate::settings::{SettingsDomain, SettingsMetadata};
use serde::Deserialize;
use splitty_core::config::{
    AppearanceSettings, DrawerSettings, EditorSettings, NativeTabSettings, RemoteSettings,
};
use splitty_core::native_api::Operation;
use splitty_transport::budget::Charged;
use std::io;
use uuid::Uuid;

#[derive(Deserialize)]
pub(crate) struct HomeAppearance {
    pub appearance: AppearanceSettings,
    pub drawer: DrawerSettings,
    pub terminal: TerminalPreferences,
    /// Absent on a service that predates the group.
    #[serde(default)]
    pub remote: RemoteSettings,
    /// Absent on a service that predates the group.
    #[serde(default, rename = "nativeTabs")]
    pub native_tabs: NativeTabSettings,
    #[serde(default)]
    pub editors: EditorSettings,
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
            self.terminal.theme_dark(),
            self.terminal.theme_light(),
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
pub(crate) async fn read(
    client: &mut Client,
    connection_id: Uuid,
    generation: u64,
    scheme: ColorScheme,
    terminal_generation: u64,
    observed: &mut Option<SettingsDomain>,
) -> io::Result<AppearanceSnapshot> {
    let home = read_typed::<HomeAppearance>(client, Operation::HomeGet).await?;
    home.validate()?;
    *observed = Some(SettingsDomain::Appearance);
    let settings = home.map(|home| HomePresentation {
        appearance: home.appearance,
        drawer: home.drawer,
        terminal: home.terminal,
        remote: home.remote,
        native_tabs: home.native_tabs,
        editors: home.editors,
    });
    let metadata = read_typed::<SettingsMetadata>(client, Operation::HomeMetadata)
        .await
        .and_then(|metadata| {
            metadata.validate()?;
            Ok(metadata)
        });
    let (metadata, metadata_error) = match metadata {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(error.to_string())),
    };
    Ok(AppearanceSnapshot {
        connection_id,
        generation,
        settings,
        scheme,
        terminal_generation,
        metadata,
        metadata_error,
    })
}

#[cfg(all(test, not(target_os = "windows")))]
pub(crate) mod tests;
