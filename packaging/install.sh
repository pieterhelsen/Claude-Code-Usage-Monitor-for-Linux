#!/bin/sh
# Claude Code Usage Monitor for Linux: installer.
#
#   curl -fsSL https://raw.githubusercontent.com/pieterhelsen/Claude-Code-Usage-Monitor-for-Linux/main/packaging/install.sh | sh
#
# Options (after `sh -s --` when piping):
#   --version vX.Y.Z   install a specific release (default: latest)
#   --from-source      build from the checkout this script lives in (needs cargo)
#   --no-extension     skip the GNOME Shell extension
#   --uninstall        remove everything this script installed
#   --purge            with --uninstall, also delete settings and caches
#
# Everything goes into your home directory; no root access is needed.
set -eu

REPO_SLUG="pieterhelsen/Claude-Code-Usage-Monitor-for-Linux"
BIN_NAME="claude-code-usage-monitor"
BUS_NAME="io.github.pieterhelsen.ClaudeCodeUsageMonitor"
UNIT="claude-code-usage-monitor.service"
UUID="claude-code-usage-monitor@pieterhelsen.github.io"

BINDIR="${HOME}/.local/bin"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
CACHE_HOME="${XDG_CACHE_HOME:-$HOME/.cache}"
DBUS_SERVICE="$DATA_HOME/dbus-1/services/$BUS_NAME.service"
SYSTEMD_UNIT="$CONFIG_HOME/systemd/user/$UNIT"
EXTENSION_DIR="$DATA_HOME/gnome-shell/extensions/$UUID"

VERSION="latest"
FROM_SOURCE=0
WITH_EXTENSION=1
UNINSTALL=0
PURGE=0

say() { printf '%s\n' "$*"; }
step() { printf '\033[1m==>\033[0m %s\n' "$*" >&2; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift ;;
        --version=*) VERSION="${1#--version=}" ;;
        --from-source) FROM_SOURCE=1 ;;
        --no-extension) WITH_EXTENSION=0 ;;
        --uninstall) UNINSTALL=1 ;;
        --purge) PURGE=1 ;;
        -h|--help) sed -n '2,15p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
    shift
done

[ "$(uname -s)" = Linux ] || die "this installer only supports Linux"
[ "$(id -u)" -ne 0 ] || die "run as your normal user, not root: everything installs into your home directory"

has_gnome_shell() { have gnome-shell && have gnome-extensions; }

stop_daemon() {
    if have systemctl && systemctl --user is-active --quiet "$UNIT" 2>/dev/null; then
        systemctl --user stop "$UNIT" || true
    elif [ -x "$BINDIR/$BIN_NAME" ]; then
        "$BINDIR/$BIN_NAME" --quit >/dev/null 2>&1 || true
    fi
}

reload_session_services() {
    if have systemctl; then
        systemctl --user daemon-reload 2>/dev/null || true
    fi
    if have busctl; then
        busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
            org.freedesktop.DBus ReloadConfig >/dev/null 2>&1 || true
    fi
}

# Add the extension to org.gnome.shell enabled-extensions. `gnome-extensions
# enable` only knows extensions the running shell has loaded, which on
# Wayland means after the next login.
enable_extension() {
    if gnome-extensions enable "$UUID" 2>/dev/null; then
        return 0
    fi
    have gsettings || return 1
    current=$(gsettings get org.gnome.shell enabled-extensions)
    case "$current" in
        *"'$UUID'"*) return 0 ;;
        "@as []"|"[]") updated="['$UUID']" ;;
        *) updated="${current%]}, '$UUID']" ;;
    esac
    gsettings set org.gnome.shell enabled-extensions "$updated"
}

uninstall() {
    step "Stopping the daemon"
    stop_daemon
    if have systemctl; then
        systemctl --user disable "$UNIT" >/dev/null 2>&1 || true
    fi
    if has_gnome_shell && [ -d "$EXTENSION_DIR" ]; then
        step "Removing the GNOME Shell extension"
        gnome-extensions disable "$UUID" 2>/dev/null || true
        gnome-extensions uninstall "$UUID" 2>/dev/null || rm -rf "$EXTENSION_DIR"
    fi
    rm -rf "$EXTENSION_DIR"
    step "Removing files"
    rm -f "$BINDIR/$BIN_NAME" "$DBUS_SERVICE" "$SYSTEMD_UNIT"
    reload_session_services
    if [ "$PURGE" = 1 ]; then
        rm -rf "$CONFIG_HOME/claude-code-usage-monitor" "$CACHE_HOME/claude-code-usage-monitor"
        say "Removed settings and caches."
    else
        say "Settings kept in $CONFIG_HOME/claude-code-usage-monitor (use --purge to remove)."
    fi
    say "Claude Code Usage Monitor is uninstalled."
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64) echo x86_64 ;;
        aarch64|arm64) echo aarch64 ;;
        *) die "unsupported CPU architecture: $(uname -m) (x86_64 and aarch64 are supported)" ;;
    esac
}

download() {
    url="$1"; dest="$2"
    if have curl; then
        curl -fsSL --retry 3 -o "$dest" "$url"
    elif have wget; then
        wget -q -O "$dest" "$url"
    else
        die "curl or wget is required"
    fi
}

fetch_release() {
    work="$1"
    arch=$(detect_arch)
    asset="$BIN_NAME-linux-$arch.tar.gz"
    if [ "$VERSION" = latest ]; then
        base="https://github.com/$REPO_SLUG/releases/latest/download"
    else
        base="https://github.com/$REPO_SLUG/releases/download/$VERSION"
    fi
    step "Downloading $asset ($VERSION)"
    download "$base/$asset" "$work/$asset" || die "download failed: $base/$asset"
    download "$base/SHA256SUMS" "$work/SHA256SUMS" || die "download failed: $base/SHA256SUMS"
    step "Verifying checksum"
    have sha256sum || die "sha256sum is required to verify the download"
    (cd "$work" && grep " $asset\$" SHA256SUMS | sha256sum -c --quiet -) \
        || die "checksum mismatch for $asset"
    tar -xzf "$work/$asset" -C "$work"
    [ -d "$work/$BIN_NAME" ] || die "unexpected archive layout"
    echo "$work/$BIN_NAME"
}

build_from_source() {
    work="$1"
    root=$(cd "$(dirname "$0")/.." 2>/dev/null && pwd) || die "--from-source must run from a checkout"
    [ -f "$root/Cargo.toml" ] || die "--from-source must run from a checkout (no Cargo.toml in $root)"
    have cargo || die "cargo is required for --from-source (https://rustup.rs)"
    step "Building from $root"
    (cd "$root" && cargo build --release --locked) >&2
    stage="$work/$BIN_NAME"
    mkdir -p "$stage"
    cp "$root/target/release/$BIN_NAME" "$stage/"
    cp "$root"/packaging/*.in "$stage/"
    if [ "$WITH_EXTENSION" = 1 ]; then
        "$root/packaging/build-extension-zip.sh" "$stage/gnome-extension.zip" >/dev/null
    fi
    echo "$stage"
}

install_files() {
    stage="$1"
    step "Installing the daemon to $BINDIR"
    stop_daemon
    mkdir -p "$BINDIR" "$(dirname "$DBUS_SERVICE")" "$(dirname "$SYSTEMD_UNIT")"
    install -m 755 "$stage/$BIN_NAME" "$BINDIR/$BIN_NAME"
    sed "s|@BINDIR@|$BINDIR|g" "$stage/$BUS_NAME.service.in" > "$DBUS_SERVICE"
    sed "s|@BINDIR@|$BINDIR|g" "$stage/$UNIT.in" > "$SYSTEMD_UNIT"
    reload_session_services
    if have systemctl; then
        systemctl --user enable "$UNIT" >/dev/null 2>&1 || true
        systemctl --user start "$UNIT" 2>/dev/null || true
    fi
    if "$BINDIR/$BIN_NAME" --refresh >/dev/null 2>&1; then
        say "   daemon $("$BINDIR/$BIN_NAME" --version) is running"
    else
        warn "the daemon did not start; check: journalctl --user -u $UNIT"
    fi
}

install_extension() {
    stage="$1"
    if [ "$WITH_EXTENSION" = 0 ]; then
        return
    fi
    if ! has_gnome_shell; then
        say "GNOME Shell not found: skipping the panel extension."
        say "For waybar and other bars, see: $BIN_NAME --help"
        return
    fi
    [ -f "$stage/gnome-extension.zip" ] || die "the release is missing gnome-extension.zip"
    shell_major=$(gnome-shell --version | sed -n 's/^GNOME Shell \([0-9]*\).*/\1/p')
    if [ -n "$shell_major" ] && [ "$shell_major" -lt 46 ]; then
        warn "GNOME Shell $shell_major is older than 46; the extension needs 46 or newer"
        return
    fi
    step "Installing the GNOME Shell extension"
    gnome-extensions install --force "$stage/gnome-extension.zip"
    if [ -d "$EXTENSION_DIR/schemas" ] && have glib-compile-schemas; then
        glib-compile-schemas "$EXTENSION_DIR/schemas"
    fi
    if ! enable_extension; then
        warn "could not enable the extension; enable it in the Extensions app"
    fi
    if [ "${XDG_SESSION_TYPE:-}" = wayland ]; then
        say ""
        say "   Log out and back in to load the extension (GNOME on Wayland"
        say "   only loads new or updated extensions at login)."
    else
        say "   Restart GNOME Shell (Alt+F2, r, Enter) or log out and in to load it."
    fi
}

if [ "$UNINSTALL" = 1 ]; then
    uninstall
    exit 0
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

if [ "$FROM_SOURCE" = 1 ]; then
    STAGE=$(build_from_source "$WORK") || exit 1
else
    STAGE=$(fetch_release "$WORK") || exit 1
fi
install_files "$STAGE"
install_extension "$STAGE"

case ":$PATH:" in
    *":$BINDIR:"*) ;;
    *) warn "$BINDIR is not on your PATH; add it to use $BIN_NAME from a terminal" ;;
esac

say ""
say "Done. Try: $BIN_NAME --waybar"
say "Uninstall with: sh install.sh --uninstall"
