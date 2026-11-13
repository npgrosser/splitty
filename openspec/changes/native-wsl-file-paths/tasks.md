# Tasks

## 1. Files of the local distribution

- [x] 1.1 client-core `native_workspace::files`: a pure function deciding per path (the path as text, the local distribution's name or none) between inserting a Linux path and storing the file. Unit tests (unit): both UNC forms, mixed case, a different distribution, a drive path, spaces and non-ASCII names, the share root.
- [x] 1.2 gpui `workspace/file_transfer.rs`: pastes and drops into a local pane of a WSL server insert the referenced paths quoted and store the rest; a unit test of the batch split (unit). Update `docs/architecture.md`.
- [ ] 1.3 Checks from CLAUDE.md, `openspec validate --specs`.
