#!/usr/bin/env bash
# Records a demo video of wayrcursor in a fresh headless Sway.
# Usage: scripts/demo.sh [OUT_DIR]   (default: docs)
# Writes OUT_DIR/demo.mp4 (H.264) and OUT_DIR/demo.gif.
# Env: WALLPAPER (image path; default: a gradient)
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath -m "${1:-$root/docs}")
work="$root/target/demo"
rm -rf "$work"; mkdir -p "$out" "$work/frames" "$work/home/.config/wayrcursor"
export HEADLESS_RES=1280x720
export WAYRCURSOR_HEADLESS_DIR=$(mktemp -d "$XDG_RUNTIME_DIR/wayrcursor-demo.XXXXXX")
cargo build -q --manifest-path "$root/Cargo.toml" --bin wayrcursor --examples
bin="$root/target/debug"

. <("$root/scripts/headless.sh")
export WAYRCURSOR_DEVICES=wayrcursor-vshake
pids=()
cleanup() {
  kill "${pids[@]}" $(cat "$work/daemon.pid" 2>/dev/null) "$(cat "$WAYRCURSOR_HEADLESS_DIR/pid")" 2>/dev/null || true
  rm -rf "$WAYRCURSOR_HEADLESS_DIR" "$XDG_RUNTIME_DIR/wayrcursor-$WAYLAND_DISPLAY.sock"
}
trap cleanup EXIT

if [ -n "${WALLPAPER:-}" ]; then
  ffmpeg -loglevel error -y -i "$WALLPAPER" \
    -vf "scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720" -frames:v 1 "$work/wallpaper.png"
else
  ffmpeg -loglevel error -y -f lavfi -i "gradients=s=1280x720:c0=0x0f2027:c1=0x203a43:c2=0x2c5364:x0=0:y0=0:x1=1280:y1=720" \
    -frames:v 1 "$work/wallpaper.png"
fi
swaymsg -q "output HEADLESS-1 bg $work/wallpaper.png fill; seat seat0 xcursor_theme Adwaita 32; gaps inner all set 14; gaps outer all set 6"

cat > "$work/alacritty.toml" <<'TOML'
[font]
size = 11
[window]
padding = { x = 14, y = 12 }
[colors.primary]
background = "#1b1f27"
foreground = "#d8dee9"
TOML

# The narrator types each line it reads, runs it unless it is a comment, then acks.
cat > "$work/narrator.sh" <<'SH'
export HOME=$1 PATH=$2:$PATH; unset XDG_CONFIG_HOME
exec 3<>"$4/say.fifo" 4<>"$4/ack.fifo"
cd "$HOME"
prompt() { printf '\033[1;34m~\033[0m \033[1;32m$\033[0m '; }
prompt
while IFS= read -r line; do
  case $line in '#'*) printf '\033[90m' ;; esac
  for ((i = 0; i < ${#line}; i++)); do printf '%s' "${line:i:1}"; sleep 0.04; done
  sleep 0.25
  printf '\033[0m\n'
  case $line in '#'*) ;; *) eval "$line" ;; esac
  case $line in *'&') echo $! > "$3" ;; esac
  prompt
  echo done >&4
done <&3
SH

for f in say ack vp shake; do rm -f "$work/$f.fifo"; mkfifo "$work/$f.fifo"; done
exec 3<>"$work/say.fifo" 4<>"$work/ack.fifo" 5<>"$work/vp.fifo" 6<>"$work/shake.fifo"
read -r lw lh < <(swaymsg -t get_outputs | jq -r '.[0].rect | "\(.width) \(.height)"')
"$bin/examples/vpointer" "$lw" "$lh" <&5 & pids+=($!)
"$bin/examples/vshake" <&6 > "$work/vshake.log" & pids+=($!)

alacritty --config-file "$work/alacritty.toml" -T narrator \
  -e bash "$work/narrator.sh" "$work/home" "$bin" "$work/daemon.pid" "$work" 2>>"$work/alacritty.log" & pids+=($!)
sleep 0.6
alacritty --config-file "$work/alacritty.toml" -T code \
  -e sh -c "printf '\033[?25l\033[?7l'; bat --paging=never --style=numbers --color=always --theme=Nord --wrap=never --line-range :42 '$root/src/ring.rs'; exec sleep infinity" 2>>"$work/alacritty.log" & pids+=($!)
sleep 1.5

x=900; y=420
echo "abs $x $y" >&5
sleep 0.3

say() { echo "$1" >&3; read -r _ <&4; }
# Eases the cursor from its position to X Y over SECONDS.
glide() {
  local steps=$(awk -v s="$3" 'BEGIN { printf "%d", s * 90 }')
  awk -v x0="$x" -v y0="$y" -v x1="$1" -v y1="$2" -v n="$steps" 'BEGIN {
    for (i = 1; i <= n; i++) { t = i / n; e = t * t * (3 - 2 * t)
      printf "abs %d %d\n", x0 + (x1 - x0) * e, y0 + (y1 - y0) * e }
  }' | while read -r line; do echo "$line" >&5; sleep 0.011; done
  x=$1; y=$2
}
# Sends N shake strokes to the daemon and wiggles the visible cursor along, drifting DX DY in total.
shake() {
  echo "shake $1" >&6
  local ddx=${2:-0} ddy=${3:-0} s i
  local steps
  read -r -a steps < <(awk -v x="$ddx" -v y="$ddy" -v n="$1" \
    'BEGIN { sx = x / (n * 10); sy = y / (n * 10); printf "%.3f,%.3f %.3f,%.3f\n", 11 + sx, sy, -11 + sx, sy }')
  for ((s = 0; s < $1; s++)); do
    local step=${steps[s % 2]}
    for ((i = 0; i < 10; i++)); do echo "rel ${step/,/ }" >&5; sleep 0.008; done
  done
  x=$((x + ddx)); y=$((y + ddy))
}

(
  i=0
  while [ ! -e "$work/stop" ]; do
    f=$(printf '%s/frames/%05d.ppm' "$work" "$i")
    grim -c -t ppm "$f"
    echo "$f $EPOCHREALTIME" >> "$work/frames.txt"
    i=$((i + 1))
  done
) & recorder=$!
sleep 0.8

glide 1050 250 1.0 & glider=$!
say "# Wayr did my cursor go?"
wait "$glider"; x=1050; y=250
say "wayrcursor daemon &"
sleep 0.3
say "wayrcursor locate"
sleep 1.3

glide 760 560 0.9
say "# Or shake the pointer"
shake 8
sleep 1.4

glide 1120 600 0.8
say "wayrcursor locate --color '#ff3366' --width 10 --radius 60"
sleep 1.4

glide 820 180 0.8
say "# The ring stays while you shake"
shake 30 -260 360
sleep 1.4

say "echo 'ring.color = \"#4ade80\"' > ~/.config/wayrcursor/config.toml"
sleep 0.4
glide 980 360 0.6
shake 8
sleep 1.6

touch "$work/stop"; wait "$recorder"

# Turns the uneven capture timestamps into frame durations for a constant-rate encode.
awk 'NR > 1 { printf "file %s\nduration %.4f\n", f, $2 - t } { f = $1; t = $2 } END { printf "file %s\n", f }' \
  "$work/frames.txt" > "$work/frames.ffconcat"
ffmpeg -loglevel error -y -f concat -safe 0 -i "$work/frames.ffconcat" -vf fps=30 \
  -c:v libx264 -preset slow -crf 20 -profile:v high -level 4.0 -pix_fmt yuv420p -movflags +faststart \
  "$out/demo.mp4"
ffmpeg -loglevel error -y -i "$out/demo.mp4" \
  -vf "fps=20,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4" \
  "$out/demo.gif"
rm -rf "$work/frames"
echo "frames: $(wc -l < "$work/frames.txt"), $(awk 'NR == 1 { a = $2 } END { printf "%.1fs", $2 - a }' "$work/frames.txt")"
ls -lh "$out/demo.mp4" "$out/demo.gif"
