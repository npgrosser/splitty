## ADDED Requirements

### Requirement: JSON Request Bodies
An API endpoint that takes a JSON body SHALL accept the request only when its `Content-Type` is `application/json` or an `application/*+json` type, with or without parameters, on the HTTP listener and on the control socket. Otherwise the server SHALL answer 415 before reading the body and without changing any state. On the HTTP listener the Host, Origin and Fetch Metadata guards and the credential check still come first. A body endpoint therefore cannot be reached by a request a browser sends across origins without a preflight (a form post or a `text/plain` body), even if a guard were missing.

#### Scenario: Text body with valid credentials
- **WHEN** an authorized same-origin request posts a well-formed operation to `/api/mux/op` with `Content-Type: text/plain`
- **THEN** the server responds 415 and the workspace state is unchanged

#### Scenario: Missing content type on the control socket
- **WHEN** a pane process posts a JSON body to `/api/session/{id}/send` over the control socket without a `Content-Type`
- **THEN** the server responds 415 and nothing is written to the session

#### Scenario: JSON with a charset
- **WHEN** a client sends a body endpoint its JSON with `Content-Type: application/json; charset=utf-8`
- **THEN** the request is processed
