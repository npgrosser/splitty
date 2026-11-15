## MODIFIED Requirements

### Requirement: Saving An Offered File
A client SHALL register for file offers on connections whose welcome announced `file-offer-v1`, and SHALL show each valid offer as a notice that names the file as plain text, its size and its server (the host's name, or this computer), with Save and Dismiss. It SHALL send nothing but `file.decline` before the user chose Save, SHALL show at most 3 offers at once, and SHALL decline malformed offers and offers beyond that limit. On Save it SHALL accept the offer, read it chunk by chunk, show progress with a way to cancel, which sends `file.decline`, and save the file under its name as a download of the window once the last chunk arrived; on `file.closed` it SHALL drop the notice. Whatever name a server offers, the client SHALL show and save it without control characters and without the bidirectional embedding, override and isolate characters (U+202A to U+202E, U+2066 to U+2069), so that a name never displays in another order than it is saved in.

#### Scenario: Save from a host
- **WHEN** a host offers `report.pdf` and the user chooses Save
- **THEN** the window reads the file from that host's connection only and saves `report.pdf` as a download

#### Scenario: Dismiss
- **WHEN** the user dismisses an offer
- **THEN** the client sends `file.decline` and reads nothing

#### Scenario: Show the saved file
- **WHEN** the window saved `report.pdf` from an offer
- **THEN** no action of its notices asks the system file manager to show the file, and neither a page nor a host learns where it was saved

#### Scenario: A name that would display reversed
- **WHEN** an older or a hostile host offers a file named `invoice` + U+202E + `fdp.exe`
- **THEN** the notice names it `invoicefdp.exe`, and Save stores it under that name
