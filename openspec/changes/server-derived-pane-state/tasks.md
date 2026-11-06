# Tasks

- [ ] 1. Add default reducer names and trusted session metadata/exit operations; preserve existing explicit names, moved sessions and managed jobs.
- [ ] 2. Derive display titles and busy events/snapshots on the server, observe unattended sessions and apply owning-host close_on_exit after output drain.
- [ ] 3. Filter server replay across frame boundaries without changing live output or byte-budget admission.
- [ ] 4. Gate frontend metadata, launch override omission and server exit ownership by capability, retaining old-host and replay protection.
- [ ] 5. Extend unit/in-process tests for all naming, inheritance, metadata, close policy, moved session, retention and replay scenarios; extend frontend unit tests for derived values and legacy behavior.
- [ ] 6. Update protocol and architecture descriptions and validate change artifacts.
- [ ] 7. Run targeted core mux/protocol/replay and server observer tests plus touched frontend tests and type checking.
- [ ] 8. Before merge run cargo fmt --all --check; cargo clippy --workspace --exclude splitty-gui --all-targets -- -D warnings; cargo test --workspace --exclude splitty-gui (architecture dependencies installed); frontend pnpm check, pnpm test and pnpm build; openspec validate --specs. Integration owner performs this once across the combined changes.
