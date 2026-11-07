## ADDED Requirements

### Requirement: Explicit Native Windows Workspace Startup
The experimental Windows workspace SHALL accept an explicit existing WSL distribution, absolute Linux backend path and isolated configuration path. It SHALL validate complete configuration before starting its owned embedded backend over inherited native pipes. Invalid or missing configuration SHALL cause a clear failure without installing software, selecting another backend or opening a fallback listener.

#### Scenario: Configured Windows workspace
- **WHEN** the Windows executable starts with valid `--wsl-distro`, `--wsl-backend` and `--wsl-config-dir` arguments
- **THEN** the native window renders real sessions from that distribution's owned embedded backend
- **AND** configuration is assigned only to that backend child and no unrelated daemon or distribution is changed

#### Scenario: Invalid or unavailable startup target
- **WHEN** required startup configuration is incomplete, malformed or unavailable
- **THEN** startup reports the failure and sends no application requests to an alternative backend
- **AND** any partially started owned helper is joined through cleanup

### Requirement: Native Windows Workspace Operation Parity
The experimental Windows workspace SHALL use the same validation, session identity, layout and application-outcome rules as the Unix development workspace for its supported terminal, Screen, settings, profile, zoom and template operations. API requests SHALL remain independent of terminal output while sharing the assigned backend lifetime.

#### Scenario: Windows terminal and API workflow
- **WHEN** a native Windows window creates splits/tabs, switches Screens and performs a supported settings or template operation
- **THEN** actual WSL terminal output/input and authoritative layouts remain usable with the same scoped identities
- **AND** an API failure or cancellation does not replay mutations or replace the terminal backend

### Requirement: Native Windows Owned Cleanup Evidence
Normal Windows preview shutdown SHALL join its owned streams and helper. The client SHALL mark cleanup complete only after a successful observed cleanup outcome. A failed or forced bridge exit SHALL be reported without claiming graceful backend cleanup. Shutdown SHALL never terminate the WSL distribution or unrelated services and processes.

#### Scenario: Normal and unsuccessful helper exit
- **WHEN** the preview closes normally after starting real WSL sessions
- **THEN** its helper and sessions exit while unrelated WSL work remains alive
- **WHEN** the helper exits unsuccessfully or cleanup cannot be established
- **THEN** the client reports the failure and does not publish a successful cleanup result

#### Scenario: Cancellation during startup
- **WHEN** the window closes while its owned helper or API stream is being established
- **THEN** admitted work remains owned until cleanup is joined and no request is delivered to a replacement backend
