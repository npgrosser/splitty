## MODIFIED Requirements

### Requirement: Explicit Native Windows Workspace Startup
The Windows workspace SHALL start its owned embedded backend in a WSL distribution over inherited native pipes. Started without arguments it SHALL use the remembered distribution, otherwise the only installed WSL 2 distribution, otherwise the one the user confirms in a dialog that offers the WSL default first, and SHALL remember a confirmed choice. Distributions that container tools keep for themselves SHALL NOT be offered, and a distribution SHALL be started for inspection only once it is the candidate. While several distributions are installed, the settings SHALL offer the choice again; a change SHALL be checked for support before it is remembered and SHALL apply at the next start without affecting running terminals. A distribution is supported when it runs WSL 2 on x86_64, is Ubuntu or Debian and its default user is not root. It SHALL install the backend that ships with the app into that distribution when the installed one of its version is missing or differs from the shipped checksum, verifying the shipped archive before use and replacing an installation atomically, and SHALL leave the distribution's configuration directory at its default. Explicit `--wsl-distro`, `--wsl-backend` and `--wsl-config-dir` arguments SHALL override this and install nothing. A startup that cannot find a supported distribution, verify or install the backend, or start it SHALL report the reason in a visible dialog, SHALL NOT select another backend or open a fallback listener, and SHALL NOT install or change WSL itself.

#### Scenario: Start without arguments
- **WHEN** the installed Windows app starts without arguments and a supported distribution exists
- **THEN** the backend of the app's version is present in that distribution, installed first when missing or different
- **AND** the native window renders real sessions from that distribution's owned embedded backend using the distribution's default configuration directory

#### Scenario: Several distributions on the first start
- **WHEN** the app starts without arguments, nothing is remembered and several WSL 2 distributions are installed
- **THEN** it asks which one runs local terminals before starting any of them
- **AND** the confirmed, supported distribution is used and remembered for later starts

#### Scenario: Changing the distribution in the settings
- **WHEN** another installed distribution is chosen in the settings
- **THEN** it is remembered only if it is supported, and the refusal names the reason otherwise
- **AND** running terminals stay in their distribution until the app starts again

#### Scenario: Backend already installed
- **WHEN** the distribution already holds the backend of the app's version with the shipped checksum
- **THEN** startup installs nothing and starts that backend

#### Scenario: Configured Windows workspace
- **WHEN** the Windows executable starts with valid `--wsl-distro`, `--wsl-backend` and `--wsl-config-dir` arguments
- **THEN** the native window renders real sessions from that distribution's owned embedded backend
- **AND** configuration is assigned only to that backend child, nothing is installed and no unrelated daemon or distribution is changed

#### Scenario: Invalid or unavailable startup target
- **WHEN** explicit startup configuration is incomplete or malformed, no supported distribution exists, or the backend cannot be verified, installed or started
- **THEN** startup shows the failure and sends no application requests to an alternative backend
- **AND** any partially started owned helper is joined through cleanup and a partial installation is removed
