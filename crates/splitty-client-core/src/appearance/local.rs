use super::{theme_name, AppearanceSnapshot, ColorScheme, HomePresentation};
use crate::api_stream::ApiStream as Client;
use crate::local_service::decode;
use serde::Deserialize;
use splitty_core::native_api::Operation;
use splitty_core::{
    config::{AppearanceSettings, DrawerSettings},
    theme::ResolvedTheme,
};
use std::io;
use uuid::Uuid;

#[derive(Deserialize)]
struct HomeAppearance {
    appearance: AppearanceSettings,
    drawer: DrawerSettings,
}

pub(crate) async fn read(
    client: &mut Client,
    connection_id: Uuid,
    generation: u64,
    scheme: ColorScheme,
) -> io::Result<AppearanceSnapshot> {
    let reply = client
        .request(Operation::HomeGet, serde_json::Map::new())
        .await?;
    if reply.uncertain {
        return Err(io::Error::other(
            "local presentation read outcome is unknown",
        ));
    }
    let settings = decode::<HomeAppearance>(client, &reply)?.map(|home| HomePresentation {
        appearance: home.appearance,
        drawer: home.drawer,
    });
    drop(reply);
    // Theme identifiers are bounded by the existing resolver. Reject oversized
    // configuration strings before allocating request arguments.
    let name = theme_name(&settings.appearance, scheme);
    if name.len() > 64 || settings.appearance.font_family.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "appearance string exceeds native limit",
        ));
    }
    let mut args = serde_json::Map::new();
    args.insert("name".into(), name.into());
    let reply = client.request(Operation::ThemeGet, args).await?;
    if reply.uncertain {
        return Err(io::Error::other("local theme read outcome is unknown"));
    }
    let theme = decode::<ResolvedTheme>(client, &reply)?;
    Ok(AppearanceSnapshot {
        connection_id,
        generation,
        settings,
        theme,
    })
}

#[cfg(all(test, not(target_os = "windows")))]
pub(crate) mod tests;
