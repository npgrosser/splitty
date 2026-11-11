# Native feature gaps

## Why

The native window lacked functions the web client has: a command palette and a shortcut list, images in terminals, viewer panes, notices for activity, automatic port forwarding, sharing local ports with a host, and floating or maximized groups on remote Screens. Each of them has a web reference and, where one exists, a main spec. The native interface may differ from the web client's; the functions may not.

## What changes

- **Command palette and shortcut list.** One centered panel lists the window's commands with their keys and filters them by words. A second lists every shortcut.
- **Remote Screens.** Floating, docking, maximizing a group and refilling an empty Screen work on remote Screens as on local ones.
- **Automatic forwarding.** With `remote.autoForwardPorts`, a port that appears on a host after its first discovery list of a connection is forwarded once, without a notice. The setting has a switch under Settings.
- **Activity notices.** With `notifications.toasts`, activity in a terminal the user is not looking at shows a notice with the way to that terminal. With `notifications.desktop`, it reaches the system while the window is in the background.
- **Images in terminals.** Images a program sends with the Kitty graphics protocol are drawn in the terminal.
- **Sharing local ports.** The ports panel of a host lists the local ports shared with it and shares another, through the local service's reverse forwards.
- **Viewer panes.** A pane of the role `viewer` shows its document: rendered Markdown with its source, text with line numbers, images, or a notice for anything else. A terminal's selection opens in a viewer.
- **Annotations and reviews.** A viewer pane lists its annotations with their threads beside the document. The user comments on a selected passage or on chosen source lines, replies, resolves and deletes; the review goes to a terminal or answers a waiting `splitty review`.
- **Managed jobs.** A job pane without a terminal says what became of its job.
- **Drawer navigation keys.** Hosts, Templates and Settings show their keys in the Drawer; the Hosts manager gets one, and each key works from the other destinations.
- **Open all Screens of a host.** One action opens every live Screen of a connected host that is not open in the window.
- **Zoom steps without a wait.** A zoom step is shown when its key is pressed; steps that wait are saved as one write.

## Not in this change

Suggested changes and the finer points of annotations listed in the tasks, rendered HTML documents, syntax highlighting, Sixel and iTerm2 images (the terminal library has neither), templates and moving panes between Screens on remote hosts, the web client's remaining settings sections, system notifications on Windows, a destination directory for dropped files and drops onto a scratchpad.

## Impact

`splitty-client-core`: `hub/shares.rs`, `native_workspace/{auto_forward,shares,viewers}.rs`, `document.rs`, `notifications.rs`, `settings.rs`, `ports.rs`. `splitty-gpui`: `workspace/{palette,viewer}.rs`, the terminal engine's image reading and the terminal's painting, the ports panel, notices and settings. No protocol or server change.
