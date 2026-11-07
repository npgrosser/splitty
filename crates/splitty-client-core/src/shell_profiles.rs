//! Service-owned shell choices. The client presents profiles, never resolves commands.
use splitty_core::shell_profiles::ShellProfilesState;
use splitty_transport::budget::Charged;
use uuid::Uuid;

pub(crate) const MAX_PROFILE_ID_BYTES: usize = 256;

/// Retain the typed decode's allocation lease while a picker holds its snapshot.
#[derive(Debug)]
pub struct ShellProfilesSnapshot {
    connection_id: Uuid,
    generation: u64,
    state: Charged<ShellProfilesState>,
}

impl ShellProfilesSnapshot {
    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn state(&self) -> &ShellProfilesState {
        &self.state
    }
}

pub(crate) async fn read(
    client: &mut crate::api_stream::ApiStream,
    connection_id: Uuid,
    generation: u64,
) -> std::io::Result<ShellProfilesSnapshot> {
    let reply = client
        .request(
            splitty_core::native_api::Operation::ShellProfilesGet,
            serde_json::Map::new(),
        )
        .await?;
    let state = crate::local_service::decode::<ShellProfilesState>(client, &reply)?;
    if state.profiles.len() > 128
        || state.default_profile_id.len() > MAX_PROFILE_ID_BYTES
        || state.system_default_profile_id.len() > MAX_PROFILE_ID_BYTES
        || state.profiles.iter().any(|profile| {
            profile.id.len() > MAX_PROFILE_ID_BYTES
                || profile.name.len() > 512
                || profile
                    .reason
                    .as_ref()
                    .is_some_and(|reason| reason.len() > 512)
        })
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "shell profiles exceed native presentation limits",
        ));
    }
    Ok(ShellProfilesSnapshot {
        connection_id,
        generation,
        state,
    })
}
