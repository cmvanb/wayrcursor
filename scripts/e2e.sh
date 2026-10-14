#!/usr/bin/env bash
# Records `wayrcursor locate` in a fresh headless Sway and checks the frames.
# Usage: scripts/e2e.sh OUT_DIR [locate args...]
# Env: HEADLESS_RES, HEADLESS_SCALE, CURSOR ("x y", logical).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath -m "$1"); shift
mkdir -p "$out"; rm -f "$out"/*.ppm "$out"/frames.txt
export WAYRCURSOR_HEADLESS_DIR="$out/sway"
cargo build -q --manifest-path "$root/Cargo.toml" --bin wayrcursor --example vpointer
bin="$root/target/debug"

. <("$root/scripts/headless.sh")
cleanup() { kill "$vp_pid" "$(cat "$out/sway/pid")" 2>/dev/null || true; }
trap cleanup EXIT
res=${HEADLESS_RES:-1920x1080}; scale=${HEADLESS_SCALE:-1}
read -r lw lh < <(swaymsg -t get_outputs | jq -r '.[0].rect | "\(.width) \(.height)"')
mkfifo "$out/vp.fifo" 2>/dev/null || true
"$bin/examples/vpointer" "$lw" "$lh" < <(tail -f "$out/vp.fifo") & vp_pid=$!
sleep 0.3
read -r cx cy <<<"${CURSOR:-500 300}"
echo "abs $cx $cy" > "$out/vp.fifo"
sleep 0.2

"$bin/wayrcursor" locate --config /dev/null "$@" & app=$!
start=$EPOCHREALTIME
i=0
while kill -0 "$app" 2>/dev/null; do
  f=$(printf '%s/%04d.ppm' "$out" "$i")
  grim -c -t ppm "$f"
  echo "$f $(awk -v a="$start" -v b="$EPOCHREALTIME" 'BEGIN { printf "%.4f", b - a }')" >> "$out/frames.txt"
  i=$((i + 1))
done
wait "$app"
grim -c -t ppm "$out/after.ppm"

fps=$(awk 'END { printf "%d", (NR - 1) / $2 }' "$out/frames.txt")
ffmpeg -loglevel error -y -framerate "$fps" -i "$out/%04d.ppm" -pix_fmt yuv420p -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" "$out/locate.mp4"
ffmpeg -loglevel error -y -framerate "$fps" -i "$out/%04d.ppm" -vf "select='not(mod(n\,3))',scale=480:-1,tile=4x4" -frames:v 1 "$out/sheet.png"
python3 "$root/scripts/check_frames.py" "$out" "$cx" "$cy" "$scale" "$@"
