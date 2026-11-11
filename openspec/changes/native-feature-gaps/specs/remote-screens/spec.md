## ADDED Requirements

### Requirement: Native Sharing Of Local Ports
The native window SHALL list a host's reverse forwards and saved rules in that host's ports panel, with Stop, Resume, Remember and Forget, and SHALL share a local port only on the user's own action there, stating before it that programs running as the user on the host can use the local service, more strongly for administrative ports. A refusal SHALL be shown in the service's words.

#### Scenario: Share a local port
- **WHEN** the user types a local port in the panel and chooses to share it
- **THEN** the window opens the reverse forward, saves its rule and lists it

### Requirement: Native Automatic Forwarding
With `remote.autoForwardPorts`, the native window SHALL forward a port that appears on a host after that connection's first discovery list, once per port and process, without a notice, and SHALL leave a paused saved rule paused.

#### Scenario: A dev server starts
- **WHEN** the setting is on and a program on a connected host starts listening
- **THEN** its port is forwarded without a further action

### Requirement: Native Groups On Remote Screens
The native window SHALL float, dock and maximize groups and refill an empty Screen on remote Screens as on local ones.

#### Scenario: Float on a host
- **WHEN** the user floats a tab of a remote Screen
- **THEN** it becomes a floating group of that Screen

### Requirement: Native Screen Work On Hosts
The native window SHALL move a pane to another Screen of its host, opened or new, and SHALL apply a saved template to the remote Screen shown or to a new Screen of its host; a Screen made this way is opened in the window. It SHALL save a remote Screen as a template kept by the local service.

#### Scenario: Move to a new Screen on a host
- **WHEN** the user moves a tab of a remote Screen to a new Screen
- **THEN** the host makes the Screen and the window opens and shows it

### Requirement: Native Ended Remote Screens
The native window SHALL take a remote Screen out of the window when its connected host no longer has it. Terminating a remote Screen SHALL ask for confirmation only when one of its terminals is busy or its state is unresolved.

#### Scenario: Terminate an idle remote Screen
- **WHEN** the user terminates a remote Screen whose terminals are idle
- **THEN** it ends without a question and leaves the window

### Requirement: Native Open All Screens Of A Host
Where a connected host with a current catalog has two or more live Screens that are not open in the window, the native window SHALL offer to open all of them in one step. The step SHALL open exactly those Screens in the catalog's order through the connection the offer was made for, show the first, leave out a Screen that ended meanwhile, and SHALL NOT be replayed after a connection change.

#### Scenario: Open the rest of a host
- **WHEN** a host has four live Screens, one of them open in the window, and the user chooses to open all
- **THEN** the three others open in the window and the first of them is shown
