## MODIFIED Requirements

### Requirement: Two Control Transports
The server SHALL serve the control API on its local Unix control socket and, when explicitly started, through a browser gateway's loopback HTTP listener, and an endpoint that is available on both transports SHALL behave identically on both, for malformed requests as for valid ones. Authorization on the HTTP listener follows `access-control`; the control socket accepts requests without any credential because access to it is restricted to the owning user. Where the socket lives, which endpoints it exposes and how it is protected are defined in `access-control` (Owner-Only Control Socket, Control Socket Route Set); a missing optional control socket SHALL not start an HTTP listener. Equivalent authorized native UI operations SHALL use the same service behavior without exposing additional control or grant routes.

#### Scenario: Same endpoint on both transports
- **WHEN** a client sends `GET /api/mux/status` over the control socket and another client sends it, authorized, over the HTTP listener
- **THEN** both receive status 200 with the same response content

#### Scenario: Control socket needs no credential
- **WHEN** a process of the owning user sends `GET /api/mux/status` over the control socket without any token or cookie
- **THEN** the server responds with status 200

#### Scenario: Malformed request on both transports
- **WHEN** a client posts a body to `/api/mux/op` over the control socket that is no JSON, or JSON without the operation's shape, or sends `GET /api/mux/neighbor` with a query argument the endpoint does not define, and another client sends the same request, authorized, over the HTTP listener
- **THEN** both receive the same 4xx status and the same JSON error object, and the workspace state is unchanged
