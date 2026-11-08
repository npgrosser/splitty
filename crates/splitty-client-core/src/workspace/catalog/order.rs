//! Relative local order uses the server; remote order belongs to opened references.
use super::*;

impl SourceWorkspace {
    pub fn set_screen_placement_supported(&mut self, supported: bool) {
        self.screen_placement_supported = supported;
    }

    pub(in crate::workspace) fn can_move_screen(&self, id: &str) -> bool {
        (self.kind() == SourceKind::Remote
            || (self.ready
                && self.catalog_supported
                && self.screen_placement_supported
                && self.catalog_initialized))
            && self.catalog.iter().any(|entry| entry.id.as_str() == id)
    }

    /// Positions are computed against the current unique, validated catalog.
    pub(in crate::workspace) fn screen_move_positions(
        &self,
        id: &str,
        before: Option<&str>,
    ) -> io::Result<(usize, usize)> {
        if !self.can_move_screen(id) {
            return Err(invalid(
                "Screen ordering is unavailable for this source or reference",
            ));
        }
        let from = self
            .catalog
            .iter()
            .position(|entry| entry.id.as_str() == id)
            .ok_or_else(|| invalid("Screen reference is no longer open"))?;
        let anchor = match before {
            Some(before) => self
                .catalog
                .iter()
                .position(|entry| entry.id.as_str() == before)
                .ok_or_else(|| invalid("Screen placement anchor is no longer open"))?,
            None => self.catalog.len(),
        };
        Ok((from, anchor.saturating_sub(usize::from(anchor > from))))
    }

    pub(crate) fn move_screen(
        &mut self,
        client: &Client,
        id: &str,
        before: Option<&str>,
    ) -> io::Result<bool> {
        self.check_connection(client)?;
        self.move_screen_with(id, before, || client.move_screen(id, before))
    }

    pub(super) fn move_screen_with(
        &mut self,
        id: &str,
        before: Option<&str>,
        send: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<bool> {
        if self.kind() != SourceKind::Local {
            return Err(invalid(
                "Remote Screen order belongs to this window's references",
            ));
        }
        let (from, to) = self.screen_move_positions(id, before)?;
        if from == to {
            return Ok(false);
        }
        let mutation = self.next_mutation()?;
        send()?;
        self.mutation = mutation;
        self.mark_catalog_dirty();
        Ok(true)
    }

    pub(in crate::workspace) fn move_opened_screen(
        &mut self,
        id: &str,
        before: Option<&str>,
    ) -> io::Result<bool> {
        if self.kind() != SourceKind::Remote {
            return Err(invalid("Local Screen order belongs to the service"));
        }
        let (from, to) = self.screen_move_positions(id, before)?;
        if from == to {
            return Ok(false);
        }
        let entry = self.catalog.remove(from);
        self.catalog.insert(to, entry);
        Ok(true)
    }
}
