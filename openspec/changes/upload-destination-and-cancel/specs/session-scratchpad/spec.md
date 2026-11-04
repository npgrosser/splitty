## MODIFIED Requirements

### Requirement: Images In The Scratchpad
When the user pastes into a scratchpad and the clipboard data holds files and no text, or drops files onto it, the client SHALL store each file on the server of the session's connection as a pane does (see `terminal-sessions`, Image Paste In A Pane and Files Into A Pane) and, when path insertion is selected, SHALL insert the returned paths into the text at the cursor, in place of a selection, several paths separated by spaces. The inserted path is an edit like typed text. A paste without a file SHALL behave as before. When the server refuses a file, or does not know the message, the client SHALL say so in a notice and insert nothing.

#### Scenario: Screenshot into the scratchpad
- **WHEN** the clipboard holds a screenshot and the user pastes into a session's scratchpad
- **THEN** the image is stored on the session's server, its path appears in the text at the cursor, and the terminal receives nothing

#### Scenario: Text in the clipboard
- **WHEN** the clipboard holds text and the user pastes into the scratchpad
- **THEN** the text is inserted and no `save_image` is sent

#### Scenario: A log dropped into the scratchpad
- **WHEN** the user drops `build.log` onto a scratchpad and confirms temporary storage
- **THEN** the file is uploaded to the session's server and its path appears at the cursor
