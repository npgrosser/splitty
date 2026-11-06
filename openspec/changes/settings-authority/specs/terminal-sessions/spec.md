# Spec Delta

## MODIFIED Requirements

### Requirement: Shell Program Selection
The server SHALL choose the program to run in this order: (1) the requested `shellProfileId`, if it names a known profile that is enabled and available; (2) the legacy `shell` path, when no `shellProfileId` is given; (3) the default profile, which is the configured default if enabled and available, otherwise the first enabled and available profile. A requested profile that is missing, disabled or unavailable SHALL fall back to the default profile. The `created` or `mux.pane_spawned` event SHALL report the effective `shellProfileId` and, when fallback occurred, a `shellProfileFallbackReason`. Profile arguments MUST be passed to the program as literal, individual arguments in their configured order without shell interpolation.

#### Scenario: Profile id takes precedence over legacy shell
- **WHEN** a client sends `create` with both a valid `shellProfileId` and a `shell` path
- **THEN** the session runs the profile's command with the profile's arguments

#### Scenario: Unknown profile id falls back to the default profile
- **WHEN** a client sends `create` with a `shellProfileId` that matches no configured profile
- **THEN** the session runs the default profile
- **AND** `created` or `mux.pane_spawned` reports the default profile's id as `shellProfileId` and `shellProfileFallbackReason` as `missing`

#### Scenario: Unavailable known profile falls back to the default profile
- **WHEN** a client sends `create` with a known disabled or unavailable `shellProfileId`
- **THEN** the session runs the default profile
- **AND** `created` or `mux.pane_spawned` reports the effective profile id and `shellProfileFallbackReason` as `disabled` or `unavailable`

#### Scenario: Legacy shell gets login arguments
- **WHEN** a client sends `create` with `shell` set to `/bin/bash` and no `shellProfileId`
- **THEN** the session runs `/bin/bash` with the login argument `-l`
- **AND** `created` carries no `shellProfileId`

#### Scenario: No usable profile
- **WHEN** no `shellProfileId` or `shell` is given and no profile is enabled and available
- **THEN** session creation fails as described in Session Creation Failure
