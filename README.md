# wayrcursor

Shows where the cursor is on Wayland.
A ring starts beyond the edges of the cursor's output,
closes in on the cursor, holds for a moment, and fades out.
Run it on demand, or keep the daemon running and shake the pointer.

## Requirements

- A compositor with `wlr-layer-shell` and `wp-viewporter`,
  such as Sway or Hyprland.
- EGL and OpenGL ES 2 through Mesa or another driver.
- Membership in the `input` group for shake detection.

The ring uses these protocols when the compositor offers them:

- `wp-fractional-scale` renders crisp edges on fractional scales.
- `wp-cursor-shape` keeps the cursor visible above the ring.
- `wlr-virtual-pointer` finds a cursor that is not moving.
  Without it, the ring appears at the first pointer motion.

## Install

```sh
make install
systemctl --user enable --now wayrcursor.service
```

`make install` puts the binary in `~/.local/bin`
and the unit in `$XDG_DATA_HOME/systemd/user`.
Set `PREFIX`, `BINDIR`, `UNITDIR` or `DESTDIR` to change the targets.
The unit starts with `graphical-session.target`.

## Use

Show the ring once:

```sh
wayrcursor locate
```

`locate` hands the request to a running daemon, so the ring appears at once.
Without a daemon it draws the ring itself.

Bind it to a key in Sway:

```text
bindsym $mod+period exec wayrcursor locate
```

Shake the pointer to show the ring while the daemon runs.
Quick strokes back and forth on one axis count as a shake.
Mice, trackpoints and one-finger touchpad strokes all work.
The ring stays on screen while the shaking continues.

The ring follows the cursor until it fades,
across outputs too.
While the ring is visible it takes pointer clicks,
for about 0.75 seconds with the default timing.

## Configure

Copy [`dist/config.toml`](dist/config.toml)
to `$XDG_CONFIG_HOME/wayrcursor/config.toml` and edit it.
The file lists every key with its default.
The daemon reloads the file when it changes.

Flags override the file for one run:

```sh
wayrcursor locate --color '#ff3366' --outline '#ffffff80' --width 10 --radius 60
```

`wayrcursor daemon` takes the same flags
as defaults for every ring it shows.

## Test

```sh
make test   # unit tests
make e2e    # recorded runs in a headless Sway
make perf   # daemon CPU cost
```

`make e2e` starts a private headless Sway for each scenario,
drives a virtual cursor, and records the screen with `grim`.
make demo   # re-record docs/demo.mp4 and docs/demo.gif
Frames, a video and a contact sheet go to `target/e2e/<scenario>`.
It needs `sway`, `grim`, `ffmpeg`, `jq` and `uv`.
The shake scenarios create a uinput device without buttons,
which your running compositor ignores.
`make demo` also needs `alacritty` and `bat`.
Set `WALLPAPER` to an image path to replace the default gradient.
