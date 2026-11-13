# Spec Delta

## ADDED Requirements

### Requirement: Windows Startup Progress
The native Windows app started without a WSL target SHALL open its window while it prepares its WSL backend, not after it. Until its workspace is shown, the window SHALL name the current phase as plain text: choosing a distribution, starting it, installing the backend, starting the backend, connecting. A failure before the workspace is shown SHALL stay in the window with its reason and the phase it ended; the app SHALL NOT exit on its own because of it.

#### Scenario: Start while WSL is not running
- **WHEN** the app starts without arguments and the remembered distribution is not running
- **THEN** the window shows that the distribution is being started, then that the backend starts and connects, and then the workspace

#### Scenario: First start of a version
- **WHEN** the distribution lacks the backend of the app's version
- **THEN** the window shows that the backend is being installed in that distribution before it is started

#### Scenario: A phase fails
- **WHEN** the distribution cannot be used or the backend cannot be installed or started
- **THEN** the window shows the reason and the phase that failed, and the app keeps running until the window is closed

#### Scenario: Window closed during the preparation
- **WHEN** the window is closed while a `wsl.exe` call of the preparation runs
- **THEN** the app ends that call and exits without starting the backend
