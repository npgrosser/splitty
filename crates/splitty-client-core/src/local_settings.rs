//! One owned API stream for the two local settings reads, separate from PTY events.
use crate::{
    appearance::{AppearanceSnapshot, ColorScheme},
    shell_profiles::ShellProfilesSnapshot,
};
use serde::de::DeserializeOwned;
use splitty_server::native::client::Client;
use splitty_transport::{budget::Charged, codec, wire};
use std::{future::Future, io, pin::Pin, sync::Arc};
use tokio::sync::Notify;
use uuid::Uuid;

pub enum LocalSettingsUpdate {
    Appearance(io::Result<Arc<AppearanceSnapshot>>),
    ShellProfiles(io::Result<Arc<ShellProfilesSnapshot>>),
}

#[derive(Clone, Copy)]
enum Feature {
    Appearance,
    Profiles,
}
impl Feature {
    fn index(self) -> usize {
        match self {
            Self::Appearance => 0,
            Self::Profiles => 1,
        }
    }
    fn other(self) -> Self {
        match self {
            Self::Appearance => Self::Profiles,
            Self::Profiles => Self::Appearance,
        }
    }
}
type Read = Pin<Box<dyn Future<Output = (Client, Feature, u64, LocalSettingsUpdate)> + Send>>;

/// `next_update` may be cancelled without cancelling the retained request. Each
/// feature coalesces its own refreshes; completing even a superseded read gives
/// the other dirty feature priority. No request is retried without a refresh.
pub struct LocalSettingsReader {
    client: Option<Client>,
    pending: Option<Read>,
    cancel: Arc<Notify>,
    connection_id: Uuid,
    generations: [u64; 2],
    dirty: [bool; 2],
    next: Feature,
    scheme: ColorScheme,
}
impl LocalSettingsReader {
    pub(crate) fn new(client: Client, connection_id: Uuid, scheme: ColorScheme) -> Self {
        Self {
            client: Some(client),
            pending: None,
            cancel: Arc::new(Notify::new()),
            connection_id,
            generations: [1; 2],
            dirty: [true; 2],
            next: Feature::Appearance,
            scheme,
        }
    }
    fn check_owner(&self, owner: Uuid) -> io::Result<()> {
        if owner != self.connection_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "local settings source changed",
            ));
        }
        Ok(())
    }
    pub fn refresh_settings(&mut self, owner: Uuid) -> io::Result<()> {
        self.check_owner(owner)?;
        let generations = [
            increment(self.generations[0])?,
            increment(self.generations[1])?,
        ];
        self.generations = generations;
        self.dirty = [true; 2];
        Ok(())
    }
    pub fn set_scheme(&mut self, owner: Uuid, scheme: ColorScheme) -> io::Result<()> {
        self.check_owner(owner)?;
        if scheme != self.scheme {
            self.generations[0] = increment(self.generations[0])?;
            self.scheme = scheme;
            self.dirty[0] = true;
        }
        Ok(())
    }
    pub async fn next_update(&mut self) -> LocalSettingsUpdate {
        loop {
            if self.pending.is_none() {
                let feature = if self.dirty[self.next.index()] {
                    self.next
                } else if self.dirty[self.next.other().index()] {
                    self.next.other()
                } else {
                    std::future::pending::<Feature>().await
                };
                let mut client = self
                    .client
                    .take()
                    .expect("local settings reader owns its client");
                let cancel = self.cancel.clone();
                let connection = self.connection_id;
                let generation = self.generations[feature.index()];
                let scheme = self.scheme;
                self.dirty[feature.index()] = false;
                self.pending = Some(Box::pin(async move {
                    let cancelled = || {
                        io::Error::new(io::ErrorKind::Interrupted, "local settings read cancelled")
                    };
                    let result = match feature {
                        Feature::Appearance => {
                            LocalSettingsUpdate::Appearance(tokio::select! { biased;
                                _ = cancel.notified() => Err(cancelled()),
                                result = crate::appearance::local::read(&mut client, connection, generation, scheme) => result.map(Arc::new),
                            })
                        }
                        Feature::Profiles => {
                            LocalSettingsUpdate::ShellProfiles(tokio::select! { biased;
                                _ = cancel.notified() => Err(cancelled()),
                                result = crate::shell_profiles::read(&mut client, connection, generation) => result.map(Arc::new),
                            })
                        }
                    };
                    (client, feature, generation, result)
                }));
            }
            let (client, feature, generation, result) =
                self.pending.as_mut().expect("settings read started").await;
            self.pending = None;
            self.client = Some(client);
            self.next = feature.other();
            if generation != self.generations[feature.index()] {
                continue;
            }
            return result;
        }
    }
    pub async fn finish(mut self) -> io::Result<()> {
        if let Some(pending) = self.pending.take() {
            self.cancel.notify_one();
            let (client, _, _, _) = pending.await;
            self.client = Some(client);
        }
        if let Some(client) = self.client.take() {
            client.finish().await?;
        }
        Ok(())
    }
}
fn increment(generation: u64) -> io::Result<u64> {
    generation
        .checked_add(1)
        .ok_or_else(|| io::Error::other("local settings generation exhausted"))
}

/// The reply lease remains alive during charged typed decoding; neither feature
/// copies an unadmitted JSON body. Error text never retains peer-controlled data.
pub(crate) fn decode<T: DeserializeOwned>(
    client: &Client,
    reply: &wire::Reply,
) -> io::Result<Charged<T>> {
    decode_with_budget(&client.sender().context().budget, reply)
}

pub(crate) fn decode_with_budget<T: DeserializeOwned>(
    budget: &splitty_transport::budget::ConnectionBudget,
    reply: &wire::Reply,
) -> io::Result<Charged<T>> {
    if !(200..300).contains(&reply.status) || reply.content_type != wire::ContentType::Json {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("local settings API returned status {}", reply.status),
        ));
    }
    let bytes = reply.body.as_bytes();
    if bytes.len() > 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local settings response exceeds native limit",
        ));
    }
    let lease = codec::reserve_json_typed(bytes, budget, false)?;
    let value = serde_json::from_slice(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid local settings response",
        )
    })?;
    Ok(lease.charge(value))
}

#[cfg(test)]
mod tests;
