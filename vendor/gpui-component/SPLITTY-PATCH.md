# Pinned window-shape support

`window_border_widths` exposes the current visible layout border separately
from shadow insets, so desktop headers can align controls to the outer frame
at normal, tiled and maximized edges without duplicating the border thickness.

Base: published `gpui-component` **0.9.0**. Original licenses are retained.
Crate archive SHA-256: `45bbfdba0efa392190f2fd446ec311325302eb047d5ce45dc1bcf189cb18f5eb`.
`UPSTREAM-SHA256.json` records the original published files.

Local change: `window_border` accepts a per-window corner radius, stored in
component WindowState and exposed by `set_window_corner_radius`. The visible
border and shadow round all four untiled corners; tiled corners stay square.
The wrapper publishes exactly those radii to the platform. Default radius zero
preserves upstream callers until the application opts in with renderer support.
Returning to server decorations clears the client inset and radii, including
the X11 frame-extents property, so live transitions leave no shadow padding.

The renderer patch is required: rounding the border alone does not clip opaque
children. Remove this override once upstream provides equivalent per-window
shape support and the real-window checks pass.
