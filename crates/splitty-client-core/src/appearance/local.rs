use super::{theme_name, AppearanceSnapshot, ColorScheme};
use crate::api_stream::ApiStream as Client;
use crate::local_service::decode;
use serde::Deserialize;
use splitty_core::native_api::Operation;
use splitty_core::{config::AppearanceSettings, theme::ResolvedTheme};
use std::io;
use uuid::Uuid;

#[derive(Deserialize)]
struct HomeAppearance {
    appearance: AppearanceSettings,
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
    let settings = decode::<HomeAppearance>(client, &reply)?.map(|home| home.appearance);
    drop(reply);
    // Theme identifiers are bounded by the existing resolver. Reject oversized
    // configuration strings before allocating request arguments.
    let name = theme_name(&settings, scheme);
    if name.len() > 64 || settings.font_family.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "appearance string exceeds native limit",
        ));
    }
    let mut args = serde_json::Map::new();
    args.insert("name".into(), name.into());
    let reply = client.request(Operation::ThemeGet, args).await?;
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
