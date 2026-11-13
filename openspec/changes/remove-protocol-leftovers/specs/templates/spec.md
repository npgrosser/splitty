## MODIFIED Requirements

### Requirement: Template Validation
Application SHALL reject duplicate pane references, missing definitions, unused terminal definitions, reused pane IDs, invalid ratios and floating split layouts without changing the workspace. Legacy web panes (pane type `webview`) SHALL be dropped wherever a template is read, from its file, from a request or from a host's answer: the pane goes, the layout keeps the remaining terminals with a split of one remaining child replaced by that child, and the template never loads a URL. A template left without a tiled terminal SHALL fail to apply.

#### Scenario: Invalid or legacy content
- **WHEN** an invalid template or an all-web template is applied
- **THEN** the old screen and sessions remain intact
- **AND** mixed legacy templates omit web panes and retain their terminal layout

#### Scenario: Saved template with a web pane
- **WHEN** a saved template file holds a terminal `a` and a `webview` pane `b` side by side, and a window loads it
- **THEN** the template it receives defines only `a`, its layout is the single slot `a`, and applying it starts one terminal

#### Scenario: Host of an earlier release
- **WHEN** a window loads a template from a host whose answer still holds a `webview` pane
- **THEN** the window drops that pane when it reads the answer and shows and applies the template without it
