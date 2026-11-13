# Tasks

## 1. Editor URIs

- [x] 1.1 Build editor and SFTP URIs in client-core (`native_workspace::editor_uri`) by the web client's rules: the request's file or folder kind, SSH target checks (ports and IPv6 only for SFTP, no all-hex alias for VS Code), invisible and direction characters refused; the window shows the reason when no URI can be built.
- [x] 1.2 Unit: the shared vectors `testdata/{remote-editor,editor-folders,editor-files}.json` and the kind's trailing slash. View (gpui, `ui-test-support`): extend `an_editor_request_opens_nothing_before_the_user_confirms` with the file URI of a host's request.

## 2. Offered file names

- [x] 2.1 Drop bidirectional embeddings, overrides and isolates from an offered name in client-core.
- [x] 2.2 Unit: extend `download_names_keep_only_the_last_plain_component`. In-process (router): extend `a_file_offer_is_saved_by_the_registered_attached_window` with a path whose name holds U+202E, refused as `invalid_path`.

## 3. Files into a local pane

- [x] 3.1 Decide the route in client-core (`file_route`) over whether the local server reads this app's paths; the native window stores the files on a WSL backend as a paste and inserts own paths otherwise.
- [x] 3.2 Unit: `files_reach_a_pane_where_its_server_can_read_them`. The Windows path itself is not run here; the decision is the tested function.

## 4. Review handover

- [x] 4.1 Move the policy into client-core (`annotations::review_delivery`): a paste without the final line breaks, a file instead of a paste of several lines for a program without bracketed paste.
- [x] 4.2 Read the bracketed-paste mode from the terminal engine; hand a review stored as a file over only once it is stored and its path inserted, and say when it was not sent.
- [x] 4.3 Unit: `a_review_reaches_a_terminal_without_running_its_lines`; engine: extend `paste_preserves_policy_and_submits_complete_brackets`. View (gpui): `a_review_sent_as_a_file_is_handed_over_only_once_it_is_stored`.

## 5. Settings limits

- [x] 5.1 Keep only the field's shape in `SettingsEdit::new`; validate ranges and choices against the server's metadata, and against the built-in fallback (`fallback_limits`) only without metadata; the settings editor offers the fallback choices then.
- [x] 5.2 Unit: `edits_follow_the_servers_limits`, `without_metadata_the_fallback_limits_apply`; in-process: rename and extend `missing_metadata_preserves_home_and_keeps_compatible_writes`; view (gpui): extend `missing_metadata_keeps_confirmed_values_and_the_catalog_offers_themes_by_name`.

## 6. Records

- [x] 6.1 `docs/architecture.md` (settings fallback), `docs/issues/openspec-baseline-findings.md` (the native zoom's built-in range), `openspec/open-decisions.md` (NCF-1, NCF-2).
