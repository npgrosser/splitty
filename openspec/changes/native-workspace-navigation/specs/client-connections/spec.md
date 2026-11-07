## ADDED Requirements

### Requirement: Native Local Group Maximization
The native local workspace SHALL support maximizing and restoring a tab group as window-local state. Maximization SHALL preserve the authoritative layout, pane/session identities and tab selection. Only displayed terminals SHALL claim visibility and size; hidden terminals SHALL continue receiving output. Each Screen SHALL retain its own valid maximize choice while hidden.

#### Scenario: Maximize and restore a running group
- **WHEN** a client maximizes a local tab group and later restores it
- **THEN** the authoritative layout and sessions remain unchanged
- **AND** hidden terminals continue processing output without retaining visible size claims
- **AND** restoring republishes the displayed terminals' measured sizes

#### Scenario: Navigate while maximized
- **WHEN** the client selects or creates a tab within a maximized group
- **THEN** the group remains maximized with its current selected tab
- **WHEN** the client switches away from that Screen and returns
- **THEN** its still-valid local maximize choice is restored without affecting another Screen

#### Scenario: Reveal structural changes and reconcile removed targets
- **WHEN** a split or pane move is admitted, or the client explicitly focuses a pane outside the maximized group
- **THEN** the client restores the full viewport without rewriting the authoritative layout itself
- **WHEN** a snapshot removes or replaces the maximized group
- **THEN** the client clears the invalid view reference and focuses a surviving visible pane if one exists

#### Scenario: Close includes hidden groups
- **WHEN** Screen or window close is requested while a group is maximized
- **THEN** close assessment includes all affected sessions, including groups hidden by maximization
- **AND** cancelling the decision preserves both sessions and the current viewport
