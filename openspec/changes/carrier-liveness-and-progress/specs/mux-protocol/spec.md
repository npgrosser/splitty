## MODIFIED Requirements

### Requirement: Heartbeat and Liveness
Liveness SHALL belong to the carrier of a connection; the session messages SHALL carry no heartbeat of their own.

A native connection SHALL send a transport ping every 15 seconds while none of its pings is unanswered, SHALL answer every ping of its peer with a pong carrying the same nonce, and SHALL end the connection when, at one of its 15-second checks, no pong matching its outstanding ping has arrived for 30 seconds or more since the last matching pong or, before the first, since the connection was accepted. Other frames SHALL NOT count as an answer. A peer that falls silent is therefore ended within 45 seconds.

The browser gateway SHALL send a WebSocket ping to the browser every 15 seconds while none is unanswered and SHALL close the browser connection 30 seconds after the last matching pong or, before the first, after the upgrade. Closing it SHALL end the gateway's native connection to the service for that browser.

When a connection ends by liveness, the server SHALL release it as on any other disconnect (see Disconnect Handling): its client registration, its editor and file registrations and the streams it attached.

#### Scenario: Responsive client
- **WHEN** a native client sends no application message for several minutes but answers transport pings
- **THEN** the server SHALL keep the connection and its registration

#### Scenario: Silent client
- **WHEN** a native client stops answering transport pings
- **THEN** the server SHALL end the connection within 45 seconds and release its registration

#### Scenario: Silent browser
- **WHEN** a browser stops answering the gateway's pings while the gateway's native connection to the service stays healthy
- **THEN** the gateway SHALL close the browser connection 30 seconds after the last matching pong, and the service SHALL release that browser's session

#### Scenario: Peer of the previous release
- **WHEN** a native client or host of the previous release connects
- **THEN** transport pings decide its liveness as before; no capability is involved, since session messages never carried a ping on a native connection
