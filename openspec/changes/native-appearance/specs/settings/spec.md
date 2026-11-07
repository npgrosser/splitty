## ADDED Requirements

### Requirement: Experimental Native Local Appearance
The experimental native workspace SHALL read local appearance through `home.get` and `theme.get`, applying appearance font overrides to workspace chrome and resolved colors to chrome and terminals without writing settings. Unset chrome font overrides SHALL use resolved UI theme defaults. Automatic mode SHALL select the saved light or dark theme using the local OS preference. Remote settings SHALL NOT select application appearance.

#### Scenario: Startup appearance
- **WHEN** the local appearance read succeeds
- **THEN** workspace chrome uses the supported appearance font overrides or resolved UI theme defaults
- **AND** existing and newly created terminals use the resolved terminal colors without recreating sessions or losing terminal history
- **AND** loading appearance does not submit a settings mutation

#### Scenario: Chrome font changes preserve terminal typography
- **WHEN** appearance font size or family changes, including selection of a proportional UI font
- **THEN** workspace chrome uses the supported font choice while terminal typography retains its native defaults
- **AND** the change does not reinterpret appearance font fields as terminal font settings or activate `terminal.theme`

#### Scenario: Automatic local theme
- **WHEN** automatic theme mode is configured and the local OS color preference changes
- **THEN** the matching configured light or dark theme is requested from the local service
- **AND** a remote service's settings cannot override that choice

### Requirement: Appearance Refresh and Failure Isolation
Native appearance refresh SHALL coalesce same-service `settings.changed` events, discard superseded results, preserve terminal event delivery and finish owned API work during shutdown. Read failures SHALL retain working defaults or the last valid appearance and expose a status while terminals remain usable. Unsupported presentation values SHALL use bounded native fallbacks.

#### Scenario: New settings during a refresh
- **WHEN** local settings change while an earlier appearance read is in flight
- **THEN** the superseded result is not applied and the latest refresh is eventually used
- **AND** terminal output continues through its existing stream

#### Scenario: Failed or unsupported appearance
- **WHEN** an appearance read fails or a requested font or color form is unsupported
- **THEN** terminals remain usable with the last valid appearance or native defaults and the failure is visible
- **AND** no corrective settings write is made

#### Scenario: Close during a read
- **WHEN** the owned workspace closes during an appearance request
- **THEN** outstanding appearance work is cancelled and its stream cleanup is awaited without leaking delivery allocations or altering another service
