#!/usr/bin/env bash
# Starts an isolated headless Sway for tests and screenshots. Prints env to source.
set -euo pipefail
dir="${WAYRCURSOR_HEADLESS_DIR:-${XDG_RUNTIME_DIR}/wayrcursor-headless}"
mkdir -p "$dir"
rm -f "$dir/display"
cat > "$dir/config" <<CFG
xwayland disable
output HEADLESS-1 resolution ${HEADLESS_RES:-1920x1080} scale ${HEADLESS_SCALE:-1} bg #336699 solid_color
default_border none
exec sh -c 'echo \$WAYLAND_DISPLAY > $dir/display'
CFG
env -u WAYLAND_DISPLAY -u DISPLAY \
  WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER="${WLR_RENDERER:-gles2}" \
  SWAYSOCK="$dir/sway.sock" \
  sway -c "$dir/config" >"$dir/sway.log" 2>&1 &
echo $! > "$dir/pid"
for _ in $(seq 50); do
  [ -s "$dir/display" ] && [ -S "$dir/sway.sock" ] && break
  sleep 0.1
done
printf 'export WAYLAND_DISPLAY=%s SWAYSOCK=%s\n' "$(cat "$dir/display")" "$dir/sway.sock" | tee "$dir/env"
