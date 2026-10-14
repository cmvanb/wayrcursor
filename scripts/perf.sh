#!/usr/bin/env bash
# Measures daemon CPU cost in a fresh headless Sway (release build).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cargo build -q --release --manifest-path "$root/Cargo.toml" --bin wayrcursor --examples
bin="$root/target/release"
export WAYRCURSOR_HEADLESS_DIR=$(mktemp -d "$XDG_RUNTIME_DIR/wayrcursor-perf.XXXXXX")
. <("$root/scripts/headless.sh")
pids=()
cleanup() {
  kill "${pids[@]}" "$(cat "$WAYRCURSOR_HEADLESS_DIR/pid")" 2>/dev/null || true
  rm -rf "$WAYRCURSOR_HEADLESS_DIR" "$XDG_RUNTIME_DIR/wayrcursor-$WAYLAND_DISPLAY.sock"
}
trap cleanup EXIT
fifo="$WAYRCURSOR_HEADLESS_DIR/vp.fifo"; shake="$WAYRCURSOR_HEADLESS_DIR/shake.fifo"
mkfifo "$fifo" "$shake"; exec 3<>"$fifo" 4<>"$shake"
read -r lw lh < <(swaymsg -t get_outputs | jq -r '.[0].rect | "\(.width) \(.height)"')
"$bin/examples/vpointer" "$lw" "$lh" <&3 & pids+=($!)
"$bin/examples/vshake" <&4 > /dev/null & pids+=($!)
sleep 0.3; echo "abs 500 300" > "$fifo"
config="$WAYRCURSOR_HEADLESS_DIR/config.toml"

# CPU ns summed over threads whose name matches $2. Readers of the machine's real
# devices (in-eventN) also run here, so live mouse use is excluded where it would skew.
ticks() {
  for t in /proc/$1/task/*; do
    if [[ $(cat "$t/comm") =~ $2 ]]; then awk '{ print $1 }' "$t/schedstat"; fi
  done | awk '{ t += $1 } END { printf "%d", t }'
}
ms() { awk -v t="$1" 'BEGIN { printf "%.2f ms", t / 1e6 }'; }
not_input='^([^i]|i[^n]|in[^-])'

TIMEFORMAT="standalone locate: %R s wall, %U s user, %S s sys"
time "$bin/wayrcursor" locate --config "$config"

"$bin/wayrcursor" daemon --config "$config" & daemon=$!; pids+=($daemon)
sleep 1
before=$(ticks "$daemon" "$not_input"); sleep 5; after=$(ticks "$daemon" "$not_input")
echo "daemon idle 5 s, excluding input readers: $(ms $((after - before))) CPU"

before=$(ticks "$daemon" "$not_input")
for _ in 1 2 3 4 5; do "$bin/wayrcursor" locate --config "$config"; sleep 1; done
after=$(ticks "$daemon" "$not_input")
echo "daemon per animation: $(ms $(((after - before) / 5))) CPU"

node=$(grep -l wayrcursor-vshake /sys/class/input/event*/device/name | awk -F/ '{ print $5 }')
before=$(ticks "$daemon" "^in-$node\$")
echo "drift 30" > "$shake"; sleep 19
after=$(ticks "$daemon" "^in-$node\$")
echo "daemon per motion event (300 events, no shake): $(awk -v t=$((after - before)) 'BEGIN { printf "%.1f us", t / 300 / 1e3 }') CPU"

dd if="/dev/input/$node" of=/dev/null bs=48 status=none & dd=$!; pids+=($dd)
sleep 0.3
before=$(ticks "$dd" .)
echo "drift 30" > "$shake"; sleep 19
after=$(ticks "$dd" .)
echo "floor, dd reading the same 300 events: $(awk -v t=$((after - before)) 'BEGIN { printf "%.1f us", t / 300 / 1e3 }') CPU"
grep -E "RssAnon|RssFile" "/proc/$daemon/status" | tr -s ' ' | sed 's/^/daemon /'
