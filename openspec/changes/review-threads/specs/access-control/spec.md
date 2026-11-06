## MODIFIED Requirements

### Requirement: Control Socket Route Set
The owner control socket SHALL route only these multiplexer endpoints: `GET /api/mux/state`, `GET /api/mux/screens`, `GET /api/mux/screen/{id}`, `POST /api/mux/op`, `GET /api/mux/neighbor`, `GET /api/mux/status` and `POST /api/session/{id}/send`, plus `POST /api/editor/open` for confirmed client editor requests, `POST /api/file/offer` for file offers the user saves in a window, `GET /api/viewer/comments` and `POST /api/viewer/comments/reply` for what the user handed over on viewer panes and agents' replies to it, `POST /api/viewer/review/wait` for waits for the user's review, and the job and local agent-administration endpoints specified in `control-api`. Every other path MUST answer 404 there, in particular configuration, shell profiles, themes, templates, stats, and the credential-minting endpoints `/api/session`, `/api/bootstrap` and `/api/ws-ticket`. The separate restricted agent sockets SHALL expose only their fixed subset below. No local socket SHALL mint browser or network credentials.

#### Scenario: Multiplexer status over the socket
- **WHEN** a pane process sends `GET /api/mux/status` over the owner control socket without any credential
- **THEN** the server responds 200

#### Scenario: Privileged endpoint over the socket
- **WHEN** a pane process sends `POST /api/bootstrap` or `GET /api/home/settings` over any local control socket
- **THEN** the server responds 404

#### Scenario: Job administration over the owner socket
- **WHEN** the owning user creates an agent grant through the owner control socket
- **THEN** a local restricted socket is created and no bearer token or browser credential is issued

#### Scenario: Editor request on the owner socket
- **WHEN** the owner socket receives `POST /api/editor/open`
- **THEN** it may request a local UI confirmation but cannot authorize a launch, choose a destination host, or mint credentials
- **AND** the same route on a restricted grant socket answers 404

#### Scenario: File offer on the owner socket
- **WHEN** the owner socket receives `POST /api/file/offer`
- **THEN** it may offer the file to the windows of the session, which save it only on the user's Save
- **AND** the same route on a restricted grant socket answers 404

#### Scenario: Comments on the owner socket
- **WHEN** the owner socket receives `GET /api/viewer/comments` or `POST /api/viewer/comments/reply`
- **THEN** it returns only what the user handed over on the viewer panes of the caller's screen, and adds at most an agent's reply
- **AND** the same routes and `POST /api/viewer/review/wait` on a restricted grant socket answer 404
