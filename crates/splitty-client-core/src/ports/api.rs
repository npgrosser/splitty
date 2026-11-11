//! Existing LocalHub wire contracts; no persistence or SSH ownership.
use super::*;
use crate::api_stream::{decode, ApiStream};
use serde_json::{json, Map, Value};
use splitty_core::native_api::Operation;
use splitty_transport::{budget::Charged, wire};

const MAX_MAPPINGS: usize = 4096;
fn valid_key(target: &str, remote: u16, local: u16) -> io::Result<()> {
    let key = MappingKey::new(target, remote)?;
    if key.target() != target || local == 0 {
        return Err(invalid("Invalid forwarding response identity"));
    }
    Ok(())
}
pub(super) fn validate_active(rows: &[ActiveForward]) -> io::Result<()> {
    if rows.len() > MAX_MAPPINGS {
        return Err(invalid("Too many active forwards"));
    }
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        valid_key(&row.target, row.remote, row.local)?;
        if !seen.insert((&row.target, row.remote)) {
            return Err(invalid("Duplicate active forward"));
        }
    }
    Ok(())
}
pub(super) fn validate_saved(row: &SavedForward) -> io::Result<()> {
    valid_key(&row.target, row.remote, row.local)?;
    if row.label.chars().count() > 200 {
        return Err(invalid("Oversized saved forwarding label"));
    }
    if row.last_error.as_ref().is_some_and(|error| {
        error.code.len() > 128
            || error.message.len() > 2048
            || error.hint.as_ref().is_some_and(|hint| hint.len() > 2048)
    }) {
        return Err(invalid("Oversized saved forwarding error"));
    }
    Ok(())
}
pub(super) async fn read_active(client: &mut ApiStream) -> io::Result<Charged<Vec<ActiveForward>>> {
    let reply = client.request(Operation::ForwardList, Map::new()).await?;
    if reply.uncertain {
        return Err(invalid("Uncertain active-forward read"));
    }
    let rows = decode::<Vec<ActiveForward>>(client, &reply)?;
    let _scratch = client
        .sender()
        .context()
        .budget
        .reserve(rows.len().saturating_mul(96), false)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                "Forward validation capacity exhausted",
            )
        })?;
    validate_active(&rows)?;
    Ok(rows)
}
pub(super) async fn read_saved(client: &mut ApiStream) -> io::Result<Charged<Preferences>> {
    let reply = client
        .request(Operation::ForwardPreferencesGet, Map::new())
        .await?;
    if reply.uncertain {
        return Err(invalid("Uncertain saved-forward read"));
    }
    let rows = decode::<Preferences>(client, &reply)?;
    if rows.version != 1 || rows.forwards.len() > 256 {
        return Err(invalid("Invalid saved forwarding document"));
    }
    let _scratch = client
        .sender()
        .context()
        .budget
        .reserve(rows.forwards.len().saturating_mul(96), false)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                "Saved-forward validation capacity exhausted",
            )
        })?;
    let mut seen = std::collections::HashSet::new();
    for row in &rows.forwards {
        validate_saved(row)?;
        if !seen.insert((&row.target, row.remote)) {
            return Err(invalid("Duplicate saved forward"));
        }
    }
    Ok(rows)
}
fn body(request: &ForwardRequest, local: Option<u16>, label: bool) -> Map<String, Value> {
    let mut body = json!({"target":request.key.target(),"port":request.key.remote()});
    if let Some(local) = local {
        body["local"] = json!(local);
    }
    if label {
        if let Some(enabled) = request.restore {
            body["enabled"] = json!(enabled);
        }
        if let Some(label) = &request.label {
            body["label"] = json!(label);
        }
    }
    if !label && matches!(request.action, ForwardAction::Start | ForwardAction::Open) {
        body["preserveRestore"] = json!(true);
        if request.automatic {
            body["automatic"] = json!(true);
        }
    }
    Map::from_iter([("body".into(), body)])
}
#[derive(Debug)]
struct Failure {
    outcome: ForwardOutcome,
    reason: String,
}
impl Failure {
    fn uncertain() -> Self {
        Self {
            outcome: ForwardOutcome::Uncertain,
            reason: "Forwarding result is uncertain; refresh state before retrying".into(),
        }
    }
}
fn check_reply(client: &ApiStream, reply: &wire::Reply) -> Result<(), Failure> {
    if reply.uncertain {
        return Err(Failure::uncertain());
    }
    if (200..300).contains(&reply.status) {
        return Ok(());
    }
    #[derive(Deserialize)]
    struct ErrorBody {
        error: Option<String>,
        code: Option<String>,
    }
    let reason = if reply.content_type == wire::ContentType::Json
        && reply.body.len() <= splitty_core::native_api::MAX_API_BODY
    {
        splitty_transport::codec::reserve_json_typed(
            reply.body.as_bytes(),
            &client.sender().context().budget,
            false,
        )
        .ok()
        .and_then(|_lease| {
            let error = serde_json::from_str::<ErrorBody>(&reply.body).ok()?;
            let code = error.code.as_deref().unwrap_or_default();
            let message = error
                .error
                .as_deref()
                .unwrap_or("Forwarding request failed");
            let separator = if code.is_empty() { "" } else { ": " };
            Some(
                code.chars()
                    .chain(separator.chars())
                    .chain(message.chars())
                    .take(1024)
                    .collect(),
            )
        })
    } else {
        None
    };
    Err(Failure {
        outcome: ForwardOutcome::Failed,
        reason: reason
            .unwrap_or_else(|| format!("Forwarding request returned status {}", reply.status)),
    })
}
async fn save(
    client: &mut ApiStream,
    request: &ForwardRequest,
    local: Option<u16>,
    revision: &std::sync::atomic::AtomicU64,
) -> Result<(), Failure> {
    if client.sender().context().is_closed() {
        return Err(Failure::uncertain());
    }
    let reply = client
        .request(
            Operation::ForwardPreferencesSave,
            body(request, local, true),
        )
        .await
        .map_err(|_| Failure::uncertain())?;
    check_reply(client, &reply)?;
    let saved = decode::<SavedForward>(client, &reply).map_err(|_| Failure::uncertain())?;
    validate_saved(&saved).map_err(|_| Failure::uncertain())?;
    if request
        .restore
        .is_some_and(|enabled| enabled != saved.enabled)
        || saved.target != request.key.target
        || saved.remote != request.key.remote
        || local.is_some_and(|local| local != saved.local)
        || request
            .label
            .as_ref()
            .is_some_and(|label| *label != saved.label)
    {
        return Err(Failure::uncertain());
    }
    revision.fetch_add(1, Ordering::AcqRel);
    Ok(())
}
/// Setup and cleanup are owned by the caller. A lost acknowledgement never retries.
pub(super) async fn mutate(
    client: &mut ApiStream,
    owner: SourceToken,
    id: Uuid,
    request: &ForwardRequest,
    consent: Option<&super::runtime::Consent>,
    revision: &std::sync::atomic::AtomicU64,
) -> ForwardResult {
    let mut result = ForwardResult::new(owner, id, request, ForwardOutcome::Succeeded, None);
    let action = async {
        match request.action {
            ForwardAction::Save => save(client, request, request.local, revision).await?,
            ForwardAction::Stop | ForwardAction::Forget => {
                let operation = if request.action == ForwardAction::Stop {
                    Operation::ForwardStop
                } else {
                    Operation::ForwardPreferencesForget
                };
                let reply = client
                    .request(operation, body(request, None, false))
                    .await
                    .map_err(|_| Failure::uncertain())?;
                check_reply(client, &reply)?;
                if reply.status != 204 || !reply.body.is_empty() {
                    return Err(Failure::uncertain());
                }
                revision.fetch_add(1, Ordering::AcqRel);
            }
            ForwardAction::Start | ForwardAction::Open => {
                let reply = client
                    .request(Operation::ForwardOpen, body(request, request.local, false))
                    .await
                    .map_err(|_| Failure::uncertain())?;
                check_reply(client, &reply)?;
                let active =
                    decode::<ActiveForward>(client, &reply).map_err(|_| Failure::uncertain())?;
                valid_key(&active.target, active.remote, active.local)
                    .map_err(|_| Failure::uncertain())?;
                if active.target != request.key.target
                    || active.remote != request.key.remote
                    || request.local.is_some_and(|local| local != active.local)
                    || active.state != ActiveState::Up
                {
                    return Err(Failure::uncertain());
                }
                revision.fetch_add(1, Ordering::AcqRel);
                if let Some(consent) = consent {
                    consent.expected.store(active.local, Ordering::Release);
                }
                if request.remember {
                    if let Err(failure) = save(client, request, Some(active.local), revision).await
                    {
                        result.outcome = ForwardOutcome::Partial;
                        result.reason = Some(format!(
                            "Forward is active; Remember failed: {}",
                            failure.reason
                        ));
                    }
                }
                if let Some(consent) =
                    consent.filter(|consent| consent.active.load(Ordering::Acquire))
                {
                    // A fresh passive read must confirm the opening acknowledgement.
                    // Never replay Open to repair an absent/down/changed mapping.
                    let rows = read_active(client).await.map_err(|_| Failure {
                        outcome: ForwardOutcome::Partial,
                        reason: "Forward opened, but browser opening could not be verified".into(),
                    })?;
                    if !rows.iter().any(|row| {
                        row.target == active.target
                            && row.remote == active.remote
                            && row.local == active.local
                            && row.state == ActiveState::Up
                    }) {
                        return Err(Failure {
                            outcome: ForwardOutcome::Partial,
                            reason: "Forward changed or stopped before browser opening".into(),
                        });
                    }
                    if let Some(url) = rewritten_url(request, active.local) {
                        result.open = Some(ForwardOpen {
                            _allocation: consent.allocation.clone(),
                            url,
                            terminal: request.link.as_ref().map(|(terminal, _)| terminal.clone()),
                            active: consent.active.clone(),
                        });
                    }
                }
            }
        }
        Ok::<(), Failure>(())
    }
    .await;
    if let Err(failure) = action {
        result.outcome = failure.outcome;
        result.reason = Some(match result.reason {
            Some(previous) => format!("{previous}; {}", failure.reason),
            None => failure.reason,
        });
        result.open = None;
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn closed_owner_cannot_submit_followup_remember() {
        let (mut client, task, path) = crate::hub::tests::fake_peer::scripted_operations(
            Vec::new(),
            None,
            None,
            Some(Vec::new()),
        )
        .await;
        client.sender().context().close();
        let request = ForwardRequest::open(
            MappingKey::new("vm1", 3000).unwrap(),
            None,
            true,
            Some("App".into()),
        )
        .unwrap();
        let revision = std::sync::atomic::AtomicU64::new(0);
        assert_eq!(
            save(&mut client, &request, Some(4173), &revision)
                .await
                .unwrap_err()
                .outcome,
            ForwardOutcome::Uncertain
        );
        assert_eq!(revision.load(Ordering::Acquire), 0);
        let _ = client.finish().await;
        task.await.unwrap();
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn saved_contract_accepts_existing_multiline_labels_and_runtime_errors() {
        let saved:Preferences=serde_json::from_str(r#"{"version":1,"forwards":[{"target":"old","remote":3000,"local":4173,"label":"App\nPreview","enabled":false},{"target":"new","remote":3001,"local":4174,"label":"API","enabled":true,"lastError":{"code":"local_port_conflict","message":"occupied"}}]}"#).unwrap();
        for row in &saved.forwards {
            validate_saved(row).unwrap();
        }
        assert_eq!(saved.forwards[0].label, "App\nPreview");
        assert!(saved.forwards[1].last_error.is_some());
    }
}
