#!/bin/sh
# Runs tests/live_desktop.rs inside an isolated nested KWin (virtual output, its own
# Xwayland and its own D-Bus session): nothing reaches the user's desktop.
#   crates/flow/tests/live_desktop.sh
set -e
cd "$(dirname "$0")/../../.."
CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-3} cargo test -p glisa-flow --test live_desktop --no-run 2>/dev/null
BIN=$(ls -t target/debug/deps/live_desktop-* 2>/dev/null | grep -v '\.d$' | head -1)
[ -x "$BIN" ] || BIN=$(find target -name 'live_desktop-*' -type f -perm -u+x | grep -v '\.d$' | head -1)
RUN=$(mktemp)
cat > "$RUN" <<INNER
#!/bin/sh
LO_FLOW_NESTED=1 "$BIN" --ignored --nocapture --test-threads=1 > "$RUN.out" 2>&1
echo "exit \$?" >> "$RUN.out"
INNER
chmod +x "$RUN"
# KWIN_WAYLAND_NO_PERMISSION_CHECKS: the drag test drives the nested KWin's own pointer with
# its fake-input protocol (restricted in a real session; this KWin has no real devices).
KWIN_WAYLAND_NO_PERMISSION_CHECKS=1 timeout 120 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen --socket "lo-flow-test-$$" \
    --width 1280 --height 800 --exit-with-session "$RUN" >/dev/null 2>&1 || true
cat "$RUN.out"
rm -f "$RUN" "$RUN.out"
