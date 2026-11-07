use super::*;
use crate::appearance::local::tests::fixture;
use splitty_core::shell_profiles::{
    ShellProfile, ShellProfileSource, ShellProfilesState, ShellTarget,
};
use splitty_server::{native::client::Connector, service::catalog::Role};
use std::{task::Poll, time::Duration};

fn profiles() -> ShellProfilesState {
    ShellProfilesState {
        default_profile_id: "chosen".into(),
        profiles: vec![ShellProfile {
            id: "chosen".into(),
            name: "Chosen shell".into(),
            command: "/bin/sh".into(),
            args: vec!["-l".into()],
            enabled: true,
            target: ShellTarget::Local,
            source: ShellProfileSource::Custom,
            available: true,
            reason: None,
            experimental: false,
            detected_defaults: None,
        }],
    }
}
async fn next(reader: &mut LocalServiceClient) -> LocalServiceUpdate {
    tokio::time::timeout(Duration::from_secs(5), reader.next_update())
        .await
        .unwrap()
}
async fn profile(reader: &mut LocalServiceClient) -> io::Result<Arc<ShellProfilesSnapshot>> {
    loop {
        if let LocalServiceUpdate::ShellProfiles(result) = next(reader).await {
            return result;
        }
    }
}
pub(super) async fn begin(reader: &mut LocalServiceClient) {
    let mut read = Box::pin(reader.next_update());
    std::future::poll_fn(|cx| {
        assert!(read.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn feature_generations_are_fair_and_os_changes_do_not_reload_profiles() {
    let (service, path) = fixture();
    *service.state.shell_profiles.write().unwrap() = profiles();
    let api = Connector::default()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let budget = api.sender().context().budget.clone();
    let owner = Uuid::new_v4();
    let mut reader = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    begin(&mut reader).await;
    assert!(reader.refresh_settings(Uuid::new_v4()).is_err());
    reader.refresh_settings(owner).unwrap();
    reader.set_scheme(owner, ColorScheme::Light).unwrap();
    // Superseded appearance work yields priority to profiles, rather than
    // restarting appearance indefinitely while preferences keep changing.
    let LocalServiceUpdate::ShellProfiles(result) = next(&mut reader).await else {
        panic!("profiles were starved");
    };
    let snapshot = result.unwrap();
    assert_eq!(snapshot.connection_id(), owner);
    assert_eq!(snapshot.generation(), 2);
    assert_eq!(snapshot.state().default_profile_id, "chosen");
    assert!(budget.used() > 0);
    drop(snapshot);
    let LocalServiceUpdate::Appearance(result) = next(&mut reader).await else {
        panic!("appearance missing");
    };
    let snapshot = result.unwrap();
    assert_eq!(snapshot.generation(), 3);
    assert_eq!(snapshot.scheme(), ColorScheme::Light);
    drop(snapshot);
    reader.set_scheme(owner, ColorScheme::Dark).unwrap();
    assert!(matches!(
        next(&mut reader).await,
        LocalServiceUpdate::Appearance(Ok(_))
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), reader.next_update())
            .await
            .is_err()
    );
    reader.finish().await.unwrap();
    assert_eq!(budget.used(), 0);
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn profile_limits_are_atomic_and_failure_does_not_poison_appearance() {
    let (service, path) = fixture();
    *service.state.shell_profiles.write().unwrap() = profiles();
    let api = Connector::default()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let budget = api.sender().context().budget.clone();
    let owner = Uuid::new_v4();
    let mut reader = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    let first = profile(&mut reader).await.unwrap();
    for case in 0..5 {
        let mut invalid = profiles();
        match case {
            0 => invalid.profiles = vec![invalid.profiles[0].clone(); 129],
            1 => invalid.profiles[0].id = "x".repeat(257),
            2 => invalid.profiles[0].name = "x".repeat(513),
            3 => invalid.profiles[0].reason = Some("x".repeat(513)),
            4 => invalid.default_profile_id = "x".repeat(257),
            _ => unreachable!(),
        }
        *service.state.shell_profiles.write().unwrap() = invalid;
        reader.refresh_settings(owner).unwrap();
        let a = next(&mut reader).await;
        let b = next(&mut reader).await;
        let updates = [a, b];
        assert!(updates
            .iter()
            .any(|update| matches!(update, LocalServiceUpdate::Appearance(Ok(_)))));
        assert!(updates.iter().any(|update| matches!(update, LocalServiceUpdate::ShellProfiles(Err(error)) if error.kind() == io::ErrorKind::InvalidData)));
        drop(updates);
        assert_eq!(
            first.state().profiles[0].name,
            "Chosen shell",
            "held valid snapshot is unchanged"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), reader.next_update())
                .await
                .is_err()
        );
    }
    *service.state.shell_profiles.write().unwrap() = profiles();
    service.state.config.write().unwrap().appearance.font_family = "x".repeat(4097);
    reader.refresh_settings(owner).unwrap();
    let a = next(&mut reader).await;
    let b = next(&mut reader).await;
    let updates = [a, b];
    assert!(updates
        .iter()
        .any(|update| matches!(update, LocalServiceUpdate::Appearance(Err(_)))));
    assert!(updates
        .iter()
        .any(|update| matches!(update, LocalServiceUpdate::ShellProfiles(Ok(_)))));
    drop(updates);
    drop(first);
    reader.finish().await.unwrap();
    assert_eq!(budget.used(), 0);
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn cancelled_profile_read_retains_client_and_shutdown_releases_allocations() {
    let (service, path) = fixture();
    *service.state.shell_profiles.write().unwrap() = profiles();
    let api = Connector::default()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let budget = api.sender().context().budget.clone();
    let owner = Uuid::new_v4();
    let mut reader = LocalServiceClient::new(api, owner, ColorScheme::Dark);
    assert!(matches!(
        next(&mut reader).await,
        LocalServiceUpdate::Appearance(Ok(_))
    ));
    begin(&mut reader).await;
    assert!(reader.pending.is_some());
    reader.refresh_settings(owner).unwrap();
    // The stale profile result is suppressed; fresh appearance has priority.
    assert!(matches!(
        next(&mut reader).await,
        LocalServiceUpdate::Appearance(Ok(_))
    ));
    begin(&mut reader).await;
    tokio::time::timeout(Duration::from_secs(5), reader.finish())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(budget.used(), 0);
    service.close().await.unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn malformed_profile_json_is_charged_and_rejected() {
    let (service, path) = fixture();
    let api = Connector::default()
        .embedded(service.state.clone(), Role::Main)
        .await
        .unwrap();
    let budget = api.sender().context().budget.clone();
    let reply = wire::Reply {
        id: 1,
        status: 200,
        content_type: wire::ContentType::Json,
        body: "{\"profiles\":[{\"id\":7}]}".into(),
        uncertain: false,
    };
    let api = crate::api_stream::ApiStream::from(api);
    assert!(decode::<ShellProfilesState>(&api, &reply).is_err());
    api.finish().await.unwrap();
    assert_eq!(budget.used(), 0);
    service.close().await.unwrap();
    assert!(!path.exists());
}
