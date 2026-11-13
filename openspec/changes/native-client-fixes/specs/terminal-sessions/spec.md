## ADDED Requirements

### Requirement: Files Into A Local Pane
A native window whose local server reads the window's file paths as they are SHALL insert the paths of files of this computer pasted or dropped into a local pane, quoted where necessary to remain literal shell arguments and separated by spaces, without storing the files and without asking. Where the local server runs in WSL, whose shell cannot use the window's Windows paths, the window SHALL store such files on the local server as Files Into A Pane stores a paste and insert the stored paths, without asking, since the files do not leave this computer.

#### Scenario: A local pane on Linux or macOS
- **WHEN** the user drops `/home/user/spec 1.pdf` onto a local pane of a native window on Linux
- **THEN** `'/home/user/spec 1.pdf'` is inserted at the prompt, and nothing is stored

#### Scenario: A local pane on Windows
- **WHEN** the user drops `C:\Users\user\spec.pdf` onto a local pane of a native window on Windows, whose local server runs in WSL
- **THEN** the file is stored on that local server without a question, and the stored Linux path is inserted at the prompt

## MODIFIED Requirements

### Requirement: Saving An Offered File
A client SHALL register for file offers on connections whose welcome announced `file-offer-v1`, and SHALL show each valid offer as a notice that names the file as plain text, its size and its server (the host's name, or this computer), with Save and Dismiss. It SHALL send nothing but `file.decline` before the user chose Save, SHALL show at most 3 offers at once, and SHALL decline malformed offers and offers beyond that limit. On Save it SHALL accept the offer, read it chunk by chunk, show progress with a way to cancel, which sends `file.decline`, and save the file under its name as a download of the window once the last chunk arrived; on `file.closed` it SHALL drop the notice. Whatever name a server offers, the client SHALL show and save it without control characters and without the bidirectional embedding, override and isolate characters (U+202A to U+202E, U+2066 to U+2069), so that a name never displays in another order than it is saved in. In a desktop shell, the notice that the file was saved SHALL offer to show it in the system file manager, which asks the shell to show that download (`access-control`, Showing A Saved Download); when the shell cannot, the window SHALL say so. In a browser the notice SHALL offer no such action.

#### Scenario: Save from a host
- **WHEN** a host offers `report.pdf` and the user chooses Save
- **THEN** the window reads the file from that host's connection only and saves `report.pdf` as a download

#### Scenario: Dismiss
- **WHEN** the user dismisses an offer
- **THEN** the client sends `file.decline` and reads nothing

#### Scenario: Show the saved file
- **WHEN** the desktop app saved `report.pdf` from an offer and the user chooses to show it
- **THEN** the window asks its shell to show the download it just saved, and the file manager opens with `report.pdf` selected

#### Scenario: A name that would display reversed
- **WHEN** an older or a hostile host offers a file named `invoice` + U+202E + `fdp.exe`
- **THEN** the notice names it `invoicefdp.exe`, and Save stores it under that name
