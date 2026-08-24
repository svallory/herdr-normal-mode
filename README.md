# herdr-normal-mode

A vim-like normal mode for the [Herdr](https://herdr.dev) sidebar. Press a key
to enter the mode, then walk the sidebar with `j` and `k` — every step moves
Herdr's focus too, so you browse by looking at the panes themselves rather than
at a list of names.

Spaces and agents are one continuous list: keep pressing `j` past the last
space and the cursor carries on into the agents section.

## Install

Requires the Rust toolchain (`cargo`).

```bash
herdr plugin install maedana/herdr-normal-mode
```

## Keybinding

Add to `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+space"
type = "shell"
command = "herdr plugin pane open --plugin maedana.normal-mode --entrypoint normal"
description = "normal mode"
```

`prefix+esc` is the binding you would reach for first, and `herdr config check`
accepts it — but it never fires. Herdr's prefix mode treats Esc as "cancel the
prefix" and returns before it looks at custom commands, so the binding is dead
on arrival. Pick anything but Esc.

Then reload:

```bash
herdr server reload-config
```

## Usage

Press `prefix+space` to enter normal mode — with `prefix = "ctrl+t"` that is
`ctrl+t` then `space`. A small popup appears showing `-- NORMAL --`, the row
you are on, and your position in the list.

- `j` moves down, `k` moves up; both wrap around at the ends
- Focus follows the cursor immediately — the pane behind the popup changes as
  you move
- The cursor starts on whatever had focus when you entered the mode
- `i` or `Enter` leaves the mode (`Ctrl+C` also works), and the focus stays on
  the row you stopped on

The list is read once when the mode opens and is not refreshed while it is
open, so a space or agent that appears in the meantime shows up the next time
you enter.

## Local development

```bash
herdr plugin link /path/to/herdr-normal-mode
cargo test
cargo build --release
```

With `plugin link` the pane still runs the release binary, so rebuild after
editing. To iterate without rebuilding by hand, change the pane command in
`herdr-plugin.toml` to `["cargo", "run", "--release", "--quiet"]`.
