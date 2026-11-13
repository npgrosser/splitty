# Spec Delta

## ADDED Requirements

### Requirement: SSH Questions Of The Local Service
A local service that runs `ssh` for its hub SHALL hand the questions OpenSSH asks its askpass program (passwords, key passphrases, host key confirmations) to its local clients, never to the terminal it may have been started from. `ssh.prompts.get` (HTTP `GET /api/remote/ssh-prompts`) SHALL list the pending questions with their identifier, host target, question text and whether it is a confirmation; `ssh.prompts.answer` (HTTP `POST /api/remote/ssh-prompts`, body `{id, answer}`, `answer` `null` to cancel) SHALL settle one question once and report `{accepted}`. Both are local hub operations: remote-window connections and host relays SHALL be refused them.

A service that announces `ssh-prompts-v1` in `mux.welcome` SHALL send `{"type":"ssh.prompts.changed"}` on its local main connections when a question appears or goes, and once after the greeting while questions are pending. The notification SHALL carry neither question text nor answer, MAY coalesce, and SHALL not be sent on remote-window connections or into host relays.

The native client SHALL show the questions of its local service, whether that service is embedded or the WSL backend, read with `ssh.prompts.get` after each notification, and answer through the service; it SHALL not poll a service that announces the capability, and it SHALL not hand the service an askpass program of its own (provisional, tbd). Without the capability it SHALL poll `ssh.prompts.get` at most once a second and stop at the first refused read (provisional, tbd). A question the user dismisses SHALL be answered with a cancel. The web client MAY keep polling (provisional, tbd).

#### Scenario: A password question reaches the native window
- **WHEN** `ssh` asks for a password while the local service connects a host for a native window
- **THEN** the service sends `ssh.prompts.changed`, the window reads the question with `ssh.prompts.get` and shows it
- **AND** the answer the user gives reaches the waiting `ssh` through `ssh.prompts.answer`, and the next read no longer lists the question

#### Scenario: Questions of the WSL backend
- **WHEN** the native app on Windows runs its local service in WSL and that service's `ssh` asks a question
- **THEN** the question reaches the app's window over the backend's connection like one of an embedded service, and no further app process or window is started for it

#### Scenario: Dismissed question
- **WHEN** the user cancels or closes a question
- **THEN** the client answers it with `null`, `ssh` gets no answer and the authentication is cancelled

#### Scenario: Pending before the client connected
- **WHEN** a local main client connects while a question is pending
- **THEN** it receives `ssh.prompts.changed` after the greeting and can read and answer the question

#### Scenario: Restricted connections
- **WHEN** a remote-window connection or a host relay is connected while questions appear
- **THEN** it receives no `ssh.prompts.changed`, its welcome lacks `ssh-prompts-v1`, and its `ssh.prompts.get` and `ssh.prompts.answer` are refused

#### Scenario: Older local service
- **WHEN** the native client's local service does not announce `ssh-prompts-v1`
- **THEN** the client polls `ssh.prompts.get` at most once a second and answers as above
- **AND** when the service refuses the read, the client stops polling and shows no question
