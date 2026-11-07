//! Stable source namespaces and immutable authority captured by native views.
use super::LocalWorkspace;
use std::fmt;
use uuid::Uuid;

/// A worker-assigned namespace, independent of the source's current connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(Uuid);

impl SourceId {
    pub(super) fn initial(connection: Uuid) -> Self {
        Self(connection)
    }
}
impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Authority for one assigned source connection; never refreshed in a callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceToken {
    source: SourceId,
    connection: Uuid,
}
impl SourceToken {
    pub(crate) fn initial(connection: Uuid) -> Self {
        Self::new(SourceId::initial(connection), connection)
    }
    pub(super) fn new(source: SourceId, connection: Uuid) -> Self {
        Self { source, connection }
    }
    pub fn source_id(self) -> SourceId {
        self.source
    }
    pub fn connection_id(self) -> Uuid {
        self.connection
    }
}

/// Retained Screen identity, not authority to execute an operation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScreenKey {
    source: SourceId,
    screen: String,
}
impl ScreenKey {
    pub fn source_id(&self) -> SourceId {
        self.source
    }
    pub fn screen_id(&self) -> &str {
        &self.screen
    }
    pub fn layout_key(&self, node: &str) -> LayoutKey {
        LayoutKey {
            screen: self.clone(),
            node: node.into(),
        }
    }
}
impl fmt::Display for ScreenKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.source, self.screen.len(), self.screen)
    }
}

/// A pane's view survives movement between Screens of its assigned source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PaneKey {
    source: SourceId,
    pane: String,
}
impl PaneKey {
    pub fn source_id(&self) -> SourceId {
        self.source
    }
    pub fn pane_id(&self) -> &str {
        &self.pane
    }
}
impl fmt::Display for PaneKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.source, self.pane.len(), self.pane)
    }
}

/// Geometry and element state belong to a node on one source's Screen.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LayoutKey {
    screen: ScreenKey,
    node: String,
}
impl LayoutKey {
    pub fn screen(&self) -> &ScreenKey {
        &self.screen
    }
    pub fn node_id(&self) -> &str {
        &self.node
    }
}
impl fmt::Display for LayoutKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.screen, self.node.len(), self.node)
    }
}

/// Captured terminal binding. Catalog refreshes do not revoke unchanged input.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TerminalToken {
    source: SourceToken,
    pane: String,
    session: String,
}
impl TerminalToken {
    pub(crate) fn new(source: SourceToken, pane: &str, session: &str) -> Self {
        Self {
            source,
            pane: pane.into(),
            session: session.into(),
        }
    }
    pub fn source_token(&self) -> SourceToken {
        self.source
    }
    pub fn pane_key(&self) -> PaneKey {
        PaneKey {
            source: self.source.source,
            pane: self.pane.clone(),
        }
    }
    pub fn pane_id(&self) -> &str {
        &self.pane
    }
    pub fn session_id(&self) -> &str {
        &self.session
    }
}

impl LocalWorkspace {
    pub(crate) fn validate_shown_key(&self, screen: &ScreenKey) -> std::io::Result<()> {
        if screen.source_id() != self.source_id
            || self.shown_screen_id() != Some(screen.screen_id())
        {
            return Err(super::invalid("Screen is no longer shown on this source"));
        }
        Ok(())
    }
    pub fn source_id(&self) -> SourceId {
        self.source_id
    }
    pub fn source_token(&self) -> SourceToken {
        SourceToken::new(self.source_id, self.connection_id())
    }
    pub fn screen_key(&self, screen: &str) -> ScreenKey {
        ScreenKey {
            source: self.source_id,
            screen: screen.into(),
        }
    }
    pub fn pane_key(&self, pane: &str) -> PaneKey {
        PaneKey {
            source: self.source_id,
            pane: pane.into(),
        }
    }
    pub fn terminal_token(&self, pane: &str) -> Option<TerminalToken> {
        Some(TerminalToken::new(
            self.source_token(),
            pane,
            self.session_for(pane)?,
        ))
    }
    pub fn accepts_terminal(&self, token: &TerminalToken) -> bool {
        token.source == self.source_token()
            && self.session_for(&token.pane) == Some(token.session.as_str())
            && self.pane_for(&token.session) == Some(token.pane.as_str())
    }
}
