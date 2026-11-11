# Pinned Linux platform patches

Base: published `gpui-pre-linux` **0.3.11**, Apache-2.0 (`LICENSE-APACHE`).
Crate archive SHA-256: `79cac5cbd3485c1e3db8eafd5dc69019188f5cb0a39846946af772efa386f624`.
Upstream snapshot: `zed-industries/zed` revision `f816e1f9263e3a8ef63c4baaa02e1441ce1c6038`, as declared by the package metadata.
`UPSTREAM-SHA256.json` records every original published file before the patch.

Local change: bound each X11 event dispatch pass and explicitly resume buffered
XCB events on the next event-loop turn. Foreground tasks must run during a
continuous input stream. Preserve event order, XIM handling, repeat filtering,
and Expose coalescing; carry trailing release/keymap normalization across a
batch boundary. No application queue or input routing changes.

The native two-host window regression passes with sustained input, delayed
creation confirmation, detach/reopen and normal cleanup; see the acceptance
record in `docs/gpui-spike.md` in the enclosing workspace. Remove the path override
when a reviewed upstream release supplies the same guarantee and passes these checks.

## Copied files on Wayland

Second local change, in `src/linux/wayland/clipboard.rs`: a clipboard that
offers `text/uri-list` is read as `ClipboardEntry::ExternalPaths` in addition
to its text or image. Upstream reads text or an image only, so files copied in
a file manager arrive as their names. Splitty stores pasted files on the
pane's own server and needs the files themselves. The item's text is
unchanged, so text inputs paste what they pasted before. X11 is unchanged.
`file_list_names_only_local_files` covers the parsing.

## Targeted checks

From the enclosing workspace (test-support comes from its application member):

```sh
cargo test -p splitty-gpui -p gpui-pre-linux --features splitty-gpui/ui-test-support event_
cargo clippy -p splitty-gpui -p gpui-pre-linux --all-targets --features splitty-gpui/ui-test-support -- -D warnings
```

The four added tests cover a perpetually readable event source, repeat/real
release handling at the budget boundary, keymap/client-message ordering, and
actual calloop idle fairness plus explicit wakeup without an fd event. The XIM
filter/dispatch body is unchanged; the unit test checks its ordered message
input, not a real input-method server. Actual X11 application acceptance is
required alongside these tests to establish input/output progress under typing.
