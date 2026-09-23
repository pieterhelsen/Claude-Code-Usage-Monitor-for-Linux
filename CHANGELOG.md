# Changelog

This changelog covers the Linux edition. For the history of the Windows widget
this project is forked from (up to 2.14.55), see the
[upstream changelog](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/blob/main/CHANGELOG.md).

## 3.0.0

The first Linux release.

### Added

- A background daemon, `claude-code-usage-monitor --daemon`, that publishes usage
  on the session D-Bus and is started on demand through D-Bus activation and a
  systemd user unit.
- A GNOME Shell extension for GNOME 46 to 50. It shows a native top-bar label
  with warning colours, an optional usage bar, and a menu of every usage window.
  Its preferences cover both the panel and the daemon.
- `--json` and `--waybar` output for scripts and tiling-window-manager bars.
- A `curl | sh` installer that verifies release checksums, with `--from-source`
  and `--uninstall` modes.
- Provider errors for Cursor, OpenCode Go and Grok are now reported per provider
  instead of only in the log.

### Changed

- Settings live in `~/.config/claude-code-usage-monitor/` and caches in
  `~/.cache/claude-code-usage-monitor/`.
- Expired Claude Code, Codex and Grok logins are renewed by running the CLI found
  on `PATH` or in common per-user install locations.
- Cursor's login database is read with a bundled SQLite; HTTPS uses rustls with
  the system certificate store.

### Removed

- The Windows taskbar widget, themes, Theme Studio, self-updater and translations.
- Google Antigravity and the Claude desktop app's login fallback, which rely on
  Windows-only credential stores.
