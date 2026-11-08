//! Confirmation belongs to the worker; a dialog only holds its immutable plan.
use super::*;

impl Remotes<'_> {
    pub fn request_update(
        &mut self,
        window: &WindowWorkspace,
        request_id: uuid::Uuid,
        action: &HostActionTarget,
    ) -> io::Result<Arc<HostUpdatePlan>> {
        if let Some(plan) = &self.update_plan {
            if plan.request_id() == request_id
                && plan.target() == action
                && window.accepts_host_update_plan(plan)
            {
                return Ok(plan.clone());
            }
        }
        self.update_plan = None;
        let index = self.index(window, action)?;
        let state = window.host(action.source_id()).expect("validated host");
        if !state.is_update_available() {
            return Err(crate::connection::invalid(
                "This host has no current update offer",
            ));
        }
        let plan = Arc::new(HostUpdatePlan::new(
            request_id,
            action.clone(),
            state.update_offer.clone().expect("actionable offer"),
            window.backend_version(),
            &self.hosts[index].context.budget,
        )?);
        self.update_plan = Some(plan.clone());
        Ok(plan)
    }
    pub fn dismiss_update(&mut self, request_id: uuid::Uuid) {
        if self
            .update_plan
            .as_ref()
            .is_some_and(|plan| plan.request_id() == request_id)
        {
            self.update_plan = None;
        }
    }
    pub fn confirm_update(
        &mut self,
        window: &mut WindowWorkspace,
        plan: &Arc<HostUpdatePlan>,
    ) -> io::Result<()> {
        if !self
            .update_plan
            .as_ref()
            .is_some_and(|pending| Arc::ptr_eq(pending, plan))
            || !window.accepts_host_update_plan(plan)
        {
            return Err(crate::connection::invalid(
                "Host update confirmation is no longer current",
            ));
        }
        let index = self.index(window, plan.target())?;
        self.update_plan = None;
        let host = &mut self.hosts[index];
        host.update_pending = true;
        host.retry_at = None;
        host.recovery_deadline = None;
        host.close_source();
        window.disconnect(host.action.source_id())?;
        let state = window.host_mut(&host.action).expect("validated host");
        state.update_state = HostUpdateState::Closing;
        state.update_offer = None;
        if host.setup.is_none() && host.prepared.is_none() && host.closing.is_none() {
            host.closed_due = true;
        }
        Ok(())
    }
}
