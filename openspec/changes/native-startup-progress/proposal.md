# Proposal

## Why

On Windows the native app prepared its WSL backend before it opened a window: it started the distribution (and with it the WSL VM when it was not running), checked it, installed the backend on the first start of a version, and only then created the window. A cold start showed nothing for several seconds, and a failure ended the app with a dialog. The legacy web app showed a startup page with the current phase ("Checking …", "Installing backend in …", "Starting backend in …"), which made the wait understandable. The native app should do the same, and do less serial work on the way.

## What Changes

- The Windows preparation (choosing the distribution, inspecting it, installing the backend) runs on a thread of its own that starts before the window, so WSL boots while the window is created. The workspace worker forwards its phases to the window and continues with the prepared target.
- Until its workspace arrives, a window shows a centered startup page in the style of the empty-Screen pages: a spinner, "Starting Splitty" and the phase as one line (looking for WSL distributions, starting the WSL distribution, installing the backend, starting the backend, connecting). On Linux and macOS the page shows the backend and connection phases and passes quickly.
- A failure before the workspace arrives stays on the page with its reason and the phase it ended; the app no longer exits with a dialog. A failure after the workspace arrived stays a status line as before.
- Closing the window while the WSL target is prepared quits at once: the preparation's running `wsl.exe` call is ended and no backend is started. Once the backend starts, closing waits for the worker's answer as before.
- One `wsl.exe` round trip less on every Windows start: the inspection call also translates the SSH dialog executable's Windows path with `wslpath -u`, which the backend's start did with a call of its own. When the inspection could not translate it, the start translates it as before.
- The settings' WSL section learns the running distribution when the preparation finishes, since the window now exists before it.
- The question dialog shown with several distributions, and the notice about an unusable one, stay system dialogs; they are now kept on top of the window.

Out of scope: an in-window distribution chooser, a retry action, a host entry point on the failure page (draft `native-windows-without-wsl`), remembering a single distribution to skip `wsl.exe --list`, and folding the inspection into the backend's own `wsl.exe` call.

## Capabilities

### New Capabilities

### Modified Capabilities
- `server-modes`: a requirement for what the Windows app's window shows while its WSL backend starts and when that start fails.

## Impact

- `crates/splitty-client-core/src/owned_service.rs` (startup phases, the prepared WSL target, a resolved SSH dialog path), `native_workspace.rs` (phase and failure updates).
- `crates/splitty-gpui/src/main.rs`, `startup/wsl.rs` (preparation on its own thread, cancellable `wsl.exe` calls, the inspection's dialog path), `workspace/startup_page.rs` (new), `workspace/empty_page.rs` (shared page column), `workspace.rs`, `workspace/settings.rs`.
- No protocol, data format or security invariant changes; the backend layout in the distribution is unchanged.

## Architecture impact

No new component or relationship; the descriptions of `owned_service` (client-core), `gpui_main` and `gpui_startup` (gpui) in `architecture/` name the prepared WSL target and the startup page.

## Decisions

- The preparation runs on a plain thread that `main` starts before the window, not inside the workspace worker, so WSL boots in parallel with platform and window setup; the worker only waits for it. Closing the window stops it: every `wsl.exe` call it runs is killed once the stop flag is set, and the worker joins the thread.
- NSP-1 (provisional, tbd): a failed start keeps its page with the reason and the failed phase and offers no action; the user closes the window. The alternatives are a retry action that restarts the preparation, and a "Connect to host" entry like the legacy setup page.
- NSP-2 (provisional, tbd): the hidden Windows window keeps its existing wait for the backend's appearance, at most one second, before it shows the startup page. Showing it at once would make every cold start visible a little earlier but flash the fallback theme on warm starts.
- NSP-3 (provisional, tbd): with several distributions the choice stays a sequence of system dialogs, now topmost over the window; an in-window chooser is the alternative.
