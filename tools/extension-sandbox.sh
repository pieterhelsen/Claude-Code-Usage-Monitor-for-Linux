#!/bin/bash
# Load the GNOME Shell extension in a throwaway headless GNOME Shell and take
# screenshots, without touching the running desktop session.
#
#   tools/extension-sandbox.sh [OUTPUT_DIR]
#
# Options (environment):
#   NO_DAEMON=1         do not start the daemon (tests the "not installed" state)
#   PREFS_PAGE=providers  open the preferences on the Providers page
#   DAEMON=path         daemon binary (default: target/debug/claude-code-usage-monitor)
#
# Isolation: XDG directories, the runtime dir and GSettings (keyfile backend)
# are set *before* the private D-Bus session starts, so services it activates
# (dconf, portals, evolution) never see the real user configuration. HOME stays
# real so the daemon can read provider logins. The script refuses to run the
# inner phase unless that isolation is in place, and checks afterwards that
# ~/.config/dconf/user was not modified.
set -u

REPO=$(cd "$(dirname "$0")/.." && pwd)
UUID=claude-code-usage-monitor@pieterhelsen.github.io

if [ "${1:-}" != "--inner" ]; then
    OUT=$(realpath -m "${1:-$REPO/target/extension-sandbox}")
    mkdir -p "$OUT"
    SANDBOX=$OUT/sandbox
    rm -rf "$SANDBOX"
    mkdir -p "$SANDBOX/data" "$SANDBOX/config" "$SANDBOX/cache" "$SANDBOX/state"
    # Wayland socket paths must stay under 108 bytes.
    RUNTIME=$(mktemp -d /tmp/ccum-rt.XXXXXX)
    chmod 700 "$RUNTIME"
    DCONF_DB=$HOME/.config/dconf/user
    BEFORE=$(stat -c %Y.%s "$DCONF_DB" 2>/dev/null || echo none)
    env XDG_DATA_HOME="$SANDBOX/data" XDG_CONFIG_HOME="$SANDBOX/config" \
        XDG_CACHE_HOME="$SANDBOX/cache" XDG_RUNTIME_DIR="$RUNTIME" \
        GSETTINGS_BACKEND=keyfile DCONF_PROFILE=/dev/null \
        CLAUDE_CODE_USAGE_MONITOR_CONFIG_DIR="${CLAUDE_CODE_USAGE_MONITOR_CONFIG_DIR:-$SANDBOX/state}" \
        CCUM_OUT="$OUT" NO_DAEMON="${NO_DAEMON:-}" PREFS_PAGE="${PREFS_PAGE:-}" \
        DAEMON="${DAEMON:-$REPO/target/debug/claude-code-usage-monitor}" \
        dbus-run-session -- "$0" --inner
    STATUS=$?
    rm -rf "$RUNTIME"
    AFTER=$(stat -c %Y.%s "$DCONF_DB" 2>/dev/null || echo none)
    if [ "$BEFORE" != "$AFTER" ]; then
        echo "WARNING: $DCONF_DB changed during the run" >&2
        exit 1
    fi
    echo "Screenshots in $OUT (real dconf untouched)"
    exit $STATUS
fi

# ---- inner phase: runs inside the private bus --------------------------------
[ "${GSETTINGS_BACKEND:-}" = keyfile ] || { echo "refusing: GSettings not sandboxed" >&2; exit 1; }
case "${XDG_CONFIG_HOME:-}" in */sandbox/config) ;; *) echo "refusing: XDG_CONFIG_HOME not sandboxed" >&2; exit 1 ;; esac

EXT=$XDG_DATA_HOME/gnome-shell/extensions
mkdir -p "$EXT/$UUID" "$EXT/unsafe-helper@sandbox"
cp -r "$REPO"/gnome-extension/{metadata.json,extension.js,prefs.js,dbus.js,format.js,stylesheet.css,schemas,icons} "$EXT/$UUID/"
glib-compile-schemas "$EXT/$UUID/schemas"
if [ "$PREFS_PAGE" = providers ]; then
    sed -i 's|window.add(this._panelPage(settings));|window.add(this._providersPage(window)); window.add(this._panelPage(settings)); return;|' "$EXT/$UUID/prefs.js"
fi

# Unsafe mode lets this script call Eval and Screenshot. It only exists here.
cat > "$EXT/unsafe-helper@sandbox/metadata.json" <<'JSON'
{"uuid": "unsafe-helper@sandbox", "name": "sandbox helper", "description": "test only", "shell-version": ["46", "47", "48", "49", "50"]}
JSON
cat > "$EXT/unsafe-helper@sandbox/extension.js" <<'JS'
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
export default class extends Extension {
    enable() { global.context.unsafe_mode = true; }
    disable() {}
}
JS

gsettings set org.gnome.shell disable-user-extensions false
gsettings set org.gnome.shell enabled-extensions "['unsafe-helper@sandbox', '$UUID']"
gsettings set org.gnome.desktop.interface color-scheme prefer-dark

if [ "$NO_DAEMON" != 1 ]; then
    "$DAEMON" --daemon > "$CCUM_OUT/daemon.log" 2>&1 &
fi
gnome-shell --headless --wayland --no-x11 --virtual-monitor 1280x720 \
    --wayland-display=ccum-sandbox > "$CCUM_OUT/shell.log" 2>&1 &
SHELL_PID=$!

EVAL() { gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell --method org.gnome.Shell.Eval "$1"; }
SHOT() { gdbus call --session --dest org.gnome.Shell.Screenshot --object-path /org/gnome/Shell/Screenshot \
    --method org.gnome.Shell.Screenshot.ScreenshotArea "$@" > /dev/null; }
INDICATOR="Main.panel.statusArea['$UUID']"

for _ in $(seq 1 60); do
    EVAL "1" 2>/dev/null | grep -q true && break
    sleep 0.5
done
sleep 4
EVAL "Main.overview.hide(); true" > /dev/null
sleep 1

echo "extension: $(gnome-extensions info "$UUID" | grep -E 'State' | xargs)"
echo "panel label: $(EVAL "$INDICATOR._label.text + '  [' + $INDICATOR._label.get_style_class_name() + ']'")"
SHOT 900 0 380 32 false "$CCUM_OUT/panel.png"
EVAL "$INDICATOR.menu.open(false); true" > /dev/null
sleep 1
SHOT 640 0 640 460 false "$CCUM_OUT/menu.png"
EVAL "$INDICATOR.menu.close(false); true" > /dev/null

gnome-extensions prefs "$UUID"
sleep 4
SHOT 0 0 1280 720 false "$CCUM_OUT/prefs.png"

if [ "$NO_DAEMON" != 1 ]; then
    "$DAEMON" --quit > /dev/null 2>&1
    sleep 2
    echo "after daemon stop: $(EVAL "$INDICATOR._label.text + '  [' + $INDICATOR._label.get_style_class_name() + ']'")"
fi
kill $SHELL_PID 2> /dev/null
sleep 1
if grep -qE "JS ERROR|Unhandled promise rejection" "$CCUM_OUT/shell.log"; then
    echo "JavaScript errors (see $CCUM_OUT/shell.log):" >&2
    grep -A4 -E "JS ERROR|Unhandled promise rejection" "$CCUM_OUT/shell.log" >&2
    exit 1
fi
