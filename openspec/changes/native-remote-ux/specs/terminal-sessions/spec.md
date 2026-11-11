## ADDED Requirements

### Requirement: Native Pasted Files
The native client SHALL store images and files pasted into a terminal or scratchpad, when the paste carries no text or names copied files, on the server of that pane's connection, as the web client does (Files Into A Pane), and SHALL insert the returned paths in file order, quoted as shell arguments in a terminal. Any failure SHALL insert nothing and say why.

#### Scenario: Screenshot into a host's terminal
- **WHEN** the clipboard holds a PNG and the user pastes into a terminal of `vm1`
- **THEN** the image is stored on `vm1` and its path is pasted into that terminal, and nothing is sent to any other server

#### Scenario: Ctrl+V with an image
- **WHEN** the clipboard holds only an image and the user presses Ctrl+V in a terminal on Linux or Windows
- **THEN** the image is stored and its path pasted, and the terminal receives no `^V`

#### Scenario: Ctrl+V with text
- **WHEN** the clipboard holds text and the user presses Ctrl+V in a terminal
- **THEN** the terminal receives `^V` and nothing is stored

#### Scenario: Local file into a local pane
- **WHEN** the user pastes a copied file of this computer into a local terminal
- **THEN** its path is pasted and no copy is stored

### Requirement: Native Dropped Files
Files dropped onto a host's terminal in the native client SHALL send nothing before the user chose Send in a notice that names the host and the total size, SHALL use chunked uploads to temporary storage, and SHALL go only to a server that announces `upload-control-v1`. Files dropped onto a local terminal SHALL be inserted as their paths.

#### Scenario: Cancel a drop
- **WHEN** the user drops a file onto a terminal of `vm1` and chooses Cancel
- **THEN** no upload message is sent and nothing is pasted

#### Scenario: Host without upload control
- **WHEN** the user confirms a drop onto a host that does not announce `upload-control-v1`
- **THEN** no file bytes are sent and a notice says the host needs an update

### Requirement: Native Offered Files
The native client SHALL show a file offer as a notice with Save and Dismiss, read it only after Save, and write it under its plain name in the user's download folder without replacing an existing file. A transfer whose answer is 30 seconds overdue, or whose connection is lost, SHALL end with a notice and leave no partial file.

#### Scenario: Save an offered file twice
- **WHEN** a pane offers `report.pdf` twice and the user chooses Save both times
- **THEN** the download folder holds `report.pdf` and `report (1).pdf`, each with the offered bytes

#### Scenario: Dismiss an offer
- **WHEN** the user dismisses an offer
- **THEN** the client sends `file.decline` and reads nothing

### Requirement: Native Layout Changes Show At Once
On local and remote screens the native window SHALL show a divider drag, a tab move and a pane move when the pointer makes them, before the service answers. The service's layout SHALL replace the shown one when it rejects the change or reports a different structure. Tabs of remote screens SHALL be draggable like local ones.

#### Scenario: Divider on a remote screen
- **WHEN** the user drags a divider of a remote screen
- **THEN** the divider follows the pointer and stays where it is released

#### Scenario: Rejected move
- **WHEN** the service rejects a tab move
- **THEN** the tab is shown in its previous place again
