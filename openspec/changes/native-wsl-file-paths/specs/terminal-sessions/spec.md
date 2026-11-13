## MODIFIED Requirements

### Requirement: Files Into A Local Pane
A native window whose local server reads the window's file paths as they are SHALL insert the paths of files of this computer pasted or dropped into a local pane, quoted where necessary to remain literal shell arguments and separated by spaces, without storing the files and without asking. Where the local server runs in WSL, whose shell cannot use the window's Windows paths, the window SHALL store such files on the local server as Files Into A Pane stores a paste and insert the stored paths, without asking, since the files do not leave this computer. A file of that server's own distribution, named `\\wsl.localhost\<distro>\<rest>` or `\\wsl$\<distro>\<rest>` with `<distro>` the server's distribution in any case, SHALL instead be inserted as its Linux path `/<rest>` with forward slashes, quoted the same way, and not stored; drive-letter paths and paths of other distributions or hosts SHALL be stored. In a batch of both kinds the referenced paths SHALL be inserted at once and the stored paths after the store finishes (provisional, tbd).

#### Scenario: A local pane on Linux or macOS
- **WHEN** the user drops `/home/user/spec 1.pdf` onto a local pane of a native window on Linux
- **THEN** `'/home/user/spec 1.pdf'` is inserted at the prompt, and nothing is stored

#### Scenario: A local pane on Windows
- **WHEN** the user drops `C:\Users\user\spec.pdf` onto a local pane of a native window on Windows, whose local server runs in WSL
- **THEN** the file is stored on that local server without a question, and the stored Linux path is inserted at the prompt

#### Scenario: A file of the local server's distribution
- **WHEN** the user drops `\\wsl.localhost\Ubuntu\home\user\spec 1.pdf` onto a local pane of a native window on Windows, whose local server runs in the distribution `ubuntu`
- **THEN** `'/home/user/spec 1.pdf'` is inserted at the prompt, and nothing is stored

#### Scenario: A file of another distribution
- **WHEN** the user drops `\\wsl$\Debian\home\user\notes.txt` onto that pane
- **THEN** the file is stored on the local server and the stored path is inserted, as for a drive-letter path
