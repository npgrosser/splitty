//! Opt-in fixtures use the same bounded response contracts as production reads.
use super::*;
use crate::api_stream::decode_with_budget;
use splitty_transport::{budget::ConnectionBudget, wire};
impl ForwardSnapshot {
    #[doc(hidden)]
    pub fn ui_test_fixture(
        owner: SourceToken,
        generation: u64,
        active_json: Option<&str>,
        saved_json: Option<&str>,
    ) -> io::Result<Self> {
        let budget = ConnectionBudget::default();
        let reply = |body: &str| wire::Reply {
            id: 1,
            status: 200,
            content_type: wire::ContentType::Json,
            body: body.into(),
            uncertain: false,
        };
        let active = active_json
            .map(|json| decode_with_budget::<Vec<ActiveForward>>(&budget, &reply(json)))
            .transpose()?;
        if let Some(active) = &active {
            api::validate_active(active)?;
        }
        let saved = saved_json
            .map(|json| decode_with_budget::<Preferences>(&budget, &reply(json)))
            .transpose()?;
        if let Some(saved) = &saved {
            if saved.version != 1 || saved.forwards.len() > 256 {
                return Err(invalid("Invalid preference fixture"));
            }
            for row in &saved.forwards {
                api::validate_saved(row)?;
            }
        }
        Ok(Self {
            owner,
            generation,
            available: true,
            loading: false,
            active: active.map(Arc::new),
            saved: saved.map(Arc::new),
            active_error: active_json
                .is_none()
                .then(|| "Active forwards unavailable".into()),
            saved_error: saved_json
                .is_none()
                .then(|| "Saved forwards unavailable".into()),
        })
    }
}
impl ForwardResult {
    #[doc(hidden)]
    pub fn ui_test_fixture(
        owner: SourceToken,
        request_id: Uuid,
        request: &ForwardRequest,
        outcome: ForwardOutcome,
        reason: Option<String>,
        confirmed_local: Option<u16>,
    ) -> io::Result<Self> {
        validate_local(confirmed_local)?;
        let mut result = Self::new(owner, request_id, request, outcome, reason);
        if let Some(local) = confirmed_local {
            if request.action != ForwardAction::Open
                || !matches!(outcome, ForwardOutcome::Succeeded | ForwardOutcome::Partial)
            {
                return Err(invalid("Open fixture needs a successful opening request"));
            }
            result.open = Some(ForwardOpen {
                _allocation: None,
                url: rewritten_url(request, local).ok_or_else(|| invalid("Invalid fixture URL"))?,
                terminal: request.link.as_ref().map(|(terminal, _)| terminal.clone()),
                active: Arc::new(AtomicBool::new(true)),
            });
        }
        Ok(result)
    }
}
