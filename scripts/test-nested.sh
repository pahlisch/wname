#!/usr/bin/env bash
# Run the integration tests inside a throwaway nested Hyprland.
#
# Must be run from a Wayland session (the nested compositor opens as a
# window there). Extra arguments go to the test harness, for example:
#
#   scripts/test-nested.sh workspace
#
# Set WNAME_TEST_KEEP=1 to leave the nested Hyprland running afterwards.
set -euo pipefail

cd "$(dirname "$0")/.."
for tool in Hyprland hyprctl jq foot; do
  command -v "$tool" >/dev/null || { echo "missing: $tool" >&2; exit 1; }
done
: "${WAYLAND_DISPLAY:?run this from a Wayland session}"

cargo build --quiet

conf="$PWD/tests/hyprland-test.conf"
pid="" sig="" wl=""

cleanup() {
  if [[ -n "$pid" && "${WNAME_TEST_KEEP:-}" != 1 ]]; then
    kill "$pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

# Start a nested Hyprland and wait until its output is up and has been
# given a size by the outer compositor. Returns non-zero if that never
# happens, which some virtual GPUs do now and then.
start_nested() {
  HYPRLAND_NO_SD_NOTIFY=1 Hyprland -c "$conf" >/dev/null 2>&1 &
  pid=$!
  sig="" wl=""
  for _ in $(seq 100); do
    info=$(hyprctl instances -j | jq -r --argjson p "$pid" '.[] | select(.pid == $p) | "\(.instance) \(.wl_socket)"')
    if [[ -n "$info" ]]; then
      read -r sig wl <<<"$info"
      break
    fi
    sleep 0.1
  done
  [[ -n "$sig" ]] || return 1
  # Hyprland starts on a fallback output; wait for the real nested one.
  for _ in $(seq 150); do
    n=$(hyprctl -i "$sig" monitors -j |
      jq '[.[] | select((.name | startswith("WAYLAND")) and .width > 0)] | length')
    [[ "$n" -ge 1 ]] && return 0
    sleep 0.1
  done
  return 1
}

started=0
for attempt in 1 2 3; do
  if start_nested; then
    started=1
    break
  fi
  echo "nested Hyprland did not come up (attempt $attempt); retrying" >&2
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  pid=""
  sleep 1
done
[[ "$started" == 1 ]] || { echo "nested Hyprland never came up" >&2; exit 1; }

echo "nested Hyprland: $sig on $wl"
HYPRLAND_INSTANCE_SIGNATURE="$sig" WAYLAND_DISPLAY="$wl" WNAME_TEST_NESTED=1 \
  cargo test --test hyprland -- --ignored --test-threads=1 "$@"
