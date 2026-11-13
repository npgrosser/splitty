# Proposal

## Why

On Windows the native window stores every file pasted or dropped into a local pane on its local server in WSL, also a file that already lives in that distribution, as one dragged from Explorer's WSL view (`\\wsl.localhost\<distro>\…`, `\\wsl$\<distro>\…`). The pane then gets a copy in temporary storage instead of the file the user meant.

## What Changes

- A file of the local server's own WSL distribution, named by its `\\wsl.localhost\<distro>\…` or `\\wsl$\<distro>\…` path, goes into a local pane as its Linux path (the rest of the path with forward slashes), quoted as other inserted paths, and is not stored. The distribution is compared without regard to case.
- Every other file (drive letters, other distributions, other hosts) is stored as before; drive-letter paths are not translated.
- The decision is made per path by a platform-neutral function of client-core; a mixed batch inserts the referenced paths and stores the rest.

## Capabilities

### New Capabilities

### Modified Capabilities
- `terminal-sessions`: Files Into A Local Pane names the files of the WSL distribution that a local pane takes by path.

## Impact

- `splitty-client-core`: `native_workspace::files` gains the per-path decision.
- `splitty-gpui`: `workspace/file_transfer.rs` splits a paste or drop into referenced and stored files for a local pane in WSL.
- Docs: `docs/native-ui-handoff.md`.

## Architecture impact

none (no component or dependency changes).

## Decisions

- **WFP-1** (provisional, tbd): in a mixed batch the referenced Linux paths are inserted at once and the stored files' paths follow when the store finishes, so the inserted text lists the referenced files first. Keeping the original order would hold the referenced paths back until the store answers and lose them when it fails.
- **WFP-2** (provisional, tbd): only the two UNC forms Explorer uses are recognized (`\\wsl.localhost\` and `\\wsl$\`, any case, either slash); the extended form `\\?\UNC\wsl.localhost\…` is stored like any other path. The share root `\\wsl$\<distro>` maps to `/`, and a folder of the distribution is referenced like a file, as a local pane on Linux takes a dropped folder's path.
