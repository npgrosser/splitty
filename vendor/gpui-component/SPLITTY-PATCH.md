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

# Beveled controls

Local change: `Theme` carries an optional `bevel` (highlight, light, shadow and
dark shadow) and an optional `field` color, both unset by default and cleared
by `apply_config`. `src/bevel.rs` paints the classic two-ring 3D edge in an
element's own border space; with a bevel, `bevel_border` turns the border an
element already has into that edge and leaves the border itself transparent.
Filled and outlined buttons are raised and sink while held, selected or toggled;
text fields, number inputs, input groups, OTP cells, select and combobox
triggers and check boxes are sunken; `popover_style` surfaces, tooltips,
dialogs and sheets take the frame relief; outlined group boxes, separators in
the theme's color and menu separators are etched. A beveled check or radio box
keeps the field color when checked and draws its mark in the text color.
`input_background` returns `field` where it is set; button faces use the
previous rule. Without a bevel every component renders as before.
