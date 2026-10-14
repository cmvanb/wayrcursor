#!/usr/bin/env bash
# Records wayrcursor in a fresh headless Sway and checks the frames.
# Usage: scripts/e2e.sh OUT_DIR [style args...]
# Env:
#   MODE     locate (standalone, default) | daemon (locate via daemon) | shake | drift (slow strokes, expects no ring)
#            | reload (rewrites the config under a running daemon, expects the new color)
#   HEADLESS_RES, HEADLESS_SCALE, CURSOR ("x y", logical), MOVE_TO ("x y", sent 150ms in)
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath -m "$1"); shift
mode=${MODE:-locate}
mkdir -p "$out"; rm -f "$out"/*.ppm "$out"/frames.txt
export WAYRCURSOR_HEADLESS_DIR=$(mktemp -d "$XDG_RUNTIME_DIR/wayrcursor-e2e.XXXXXX")
cargo build -q --manifest-path "$root/Cargo.toml" --bin wayrcursor --examples
bin="$root/target/debug"
config="$WAYRCURSOR_HEADLESS_DIR/config.toml"

. <("$root/scripts/headless.sh")
pids=()
cleanup() {
  kill "${pids[@]}" "$(cat "$WAYRCURSOR_HEADLESS_DIR/pid")" 2>/dev/null || true
  rm -rf "$WAYRCURSOR_HEADLESS_DIR" "$XDG_RUNTIME_DIR/wayrcursor-$WAYLAND_DISPLAY.sock"
}
trap cleanup EXIT
scale=${HEADLESS_SCALE:-1}
read -r lw lh < <(swaymsg -t get_outputs | jq -r '.[0].rect | "\(.width) \(.height)"')
rm -f "$out/vp.fifo" "$out/shake.fifo"; mkfifo "$out/vp.fifo" "$out/shake.fifo"
# Read-write fds keep each FIFO open so its reader never sees EOF between commands.
exec 3<>"$out/vp.fifo" 4<>"$out/shake.fifo"
"$bin/examples/vpointer" "$lw" "$lh" <&3 & pids+=($!)
read -r cx cy <<<"${CURSOR:-500 300}"
sleep 0.3
echo "abs $cx $cy" > "$out/vp.fifo"

if [ "$mode" != locate ]; then
  "$bin/examples/vshake" <&4 > "$out/vshake.log" & pids+=($!)
  "$bin/wayrcursor" daemon --config "$config" "$@" & pids+=($!)
fi
sleep 0.4

case $mode in
  locate) "$bin/wayrcursor" locate --config "$config" "$@" & app=$! ;;
  daemon)
    t0=$EPOCHREALTIME
    "$bin/wayrcursor" locate --config "$config" "$@"
    # A standalone fallback would block for the whole animation.
    awk -v a="$t0" -v b="$EPOCHREALTIME" 'BEGIN { if (b - a > 0.2) { print "FAIL: locate did not hand off to the daemon"; exit 1 } }' ;;
  shake) echo "shake 8" > "$out/shake.fifo" ;;
  reload)
    printf '[ring]\ncolor = "#00ff00"\n' > "$config.tmp" && mv "$config.tmp" "$config"
    sleep 0.2
    "$bin/wayrcursor" locate --config /dev/null
    export EXPECT_COLOR=00ff00 ;;
  drift) echo "drift 8" > "$out/shake.fifo"; export EXPECT_NONE=1 ;;
esac
start=$EPOCHREALTIME
elapsed() { awk -v a="$start" -v b="$EPOCHREALTIME" 'BEGIN { printf "%.4f", b - a }'; }
recording() {
  if [ "$mode" = locate ]; then kill -0 "$app" 2>/dev/null; else awk -v t="$(elapsed)" 'BEGIN { exit !(t < 1.6) }'; fi
}
if [ -n "${MOVE_TO:-}" ]; then
  (sleep 0.15; echo "abs $MOVE_TO" > "$out/vp.fifo") &
  read -r cx cy <<<"$MOVE_TO"
  export SETTLED_AT=0.25
fi
i=0
while recording; do
  f=$(printf '%s/%04d.ppm' "$out" "$i")
  grim -c -t ppm "$f"
  echo "$f $(elapsed)" >> "$out/frames.txt"
  i=$((i + 1))
done
[ "$mode" = locate ] && wait "$app"
sleep 0.2
grim -c -t ppm "$out/after.ppm"

fps=$(awk 'END { printf "%d", (NR - 1) / $2 }' "$out/frames.txt")
ffmpeg -loglevel error -y -framerate "$fps" -i "$out/%04d.ppm" -pix_fmt yuv420p -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" "$out/recording.mp4"
ffmpeg -loglevel error -y -framerate "$fps" -i "$out/%04d.ppm" -vf "select='not(mod(n\,3))',scale=480:-1,tile=4x4" -frames:v 1 "$out/sheet.png"
uv run --quiet "$root/scripts/check_frames.py" "$out" "$cx" "$cy" "$scale" "$@"
