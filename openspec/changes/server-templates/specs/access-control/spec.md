# Spec Delta

## ADDED Requirements

### Requirement: Template Apply Authority
Template application SHALL be accepted only through authenticated window session connections. The generic `POST /api/mux/op` and native `mux.op` API SHALL reject `apply_template`, including owner control requests. Restricted agent sockets SHALL expose neither application nor extraction.

#### Scenario: Non-window template application
- **WHEN** an owner control or restricted agent caller attempts template application or extraction
- **THEN** application is denied and extraction is unavailable on those sockets
