# 5. Open the pickers as popups

Status: accepted — supersedes the placement decision in
[ADR 0004](./0004-navigator-appearance.md)

## Context

ADR 0004 chose `placement = "overlay"` and had the picker draw its own accent-coloured panel.
The reasoning was that `overlay` is a zoom, so an inset box would sit on blank space rather
than on the session, and that `popup` costs too much: a popup has no pane id, and one
measurement suggested a popup does not close when its process exits.

That last measurement was wrong. The probe set `HERDR_SOCKET_PATH` to a bad path through the
open request's `env`, expecting the picker to fail immediately; herdr applies its own
environment after the caller's, so the override never took effect and the picker simply kept
running. Re-measured with a manifest entrypoint of `sh -c 'exit 0'`, herdr closes the popup
within half a second of the process ending.

## Decision

Open both entrypoints as popups, sized to the navigator's own proportions, and stop drawing a
panel.

herdr frames a plugin popup with `Borders::ALL`, `border_style(palette.accent)`, a title from
the manifest, and `panel_bg`. That is the navigator's `render_panel_shell` — the same widget,
the same accent — drawn by the host over the live session. Drawing our own inside it would
double the border and cost two rows and two columns for nothing.

Everything the earlier decision was weighing was measured against herdr 0.7.4:

| | overlay | popup |
| --- | --- | --- |
| the session behind it | hidden (a zoom) | visible, as under the navigator |
| the frame | ours to draw | herdr's, with a title |
| in `session.snapshot` | yes | no |
| `focused_pane_id` while open | the picker | the pane it was summoned from |
| a jump made from inside | survives closing | survives closing |
| the process exiting | herdr closes it | herdr closes it |
| a second open | another overlay | `plugin_pane_open_failed: "popup already open"` |

The two facts that made this safe are the last two rows of behaviour: a `pane.focus` issued
while the popup is up still holds after it closes, so the picker's "focus, then exit" works
unchanged; and herdr tears the popup down on exit, so nothing has to call `popup.close`.

## Consequences

- **The picker is no longer a pane.** It does not appear in `session.snapshot`, so the
  exclusion that kept it out of its own list is gone, along with the plumbing that carried it
  through `tree::build`, `collect_tree`, `dest::destinations` and both event loops.
- **`focused_pane_id` stays on the invoking pane** while the picker is up, which is a more
  robust answer to "where was I summoned from" than the environment the action forwards.
  The environment is kept anyway: it is set at invocation time and cannot drift.
- **Pressing the key again cannot reach herdr.** `src/app/input/mod.rs` routes every key into
  an open popup before the prefix is considered, so the keybinding is unreachable while the
  picker is up. The "already open" refusal is therefore treated as success — the picker the
  user asked for is already in front of them — and the state file that remembered the open
  pane is gone.
- **The border title cannot change.** `Tab` switches views inside one process, and with no
  pane id nothing can rename the popup afterwards. Both entrypoints are titled
  `herdr-gh-nav` so the frame is never contradicting the screen; the search line and the key
  hint say which view you are in.
- **A popup is not addressable**, so `herdr pane read` and `herdr pane send-keys` cannot
  reach the picker. Running `herdr-gh-nav pane panes` in an ordinary pane exercises exactly
  the same code and is how the picker is checked; only the framing has to be looked at.
