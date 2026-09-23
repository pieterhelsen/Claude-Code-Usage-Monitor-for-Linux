# Changelog

This changelog covers the Linux edition, which restarts its version numbers at
1.0.0. It is forked from the Windows widget at version 2.14.55; for that history,
see the [upstream changelog](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/blob/main/CHANGELOG.md).

## 1.0.0

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
- A new login for any configured Claude Code or Codex account profile is picked
  up within 30 seconds, not only for the default login.

### Changed

- Settings live in `~/.config/claude-code-usage-monitor/` and caches in
  `~/.cache/claude-code-usage-monitor/`.
- Expired Claude Code, Codex and Grok logins are renewed by running the CLI found
  on `PATH` or in common per-user install locations.
- Cursor's login database is read with a bundled SQLite; HTTPS uses rustls with
  the system certificate store.
- OpenCode Go shows its weekly and monthly windows separately instead of
  replacing the weekly figure with the monthly one.
- A poll where every account failed is reported as failed, so `--json --local`
  exits non-zero.
- waybar's `percentage` matches the text when showing what is left.
- The top-bar label dims its last reading when the daemon stops or cannot be
  reached, instead of looking current.
- The preferences window shows the provider settings the daemon actually kept.

### Security

- Expired logins are renewed by running the provider CLI in an empty private
  directory. Claude Code runs with no tools, settings files, MCP servers or saved
  session, on Haiku. Codex runs read-only and ephemeral. Neither sees your home
  directory or a project as its workspace.
- Settings, caches and the diagnostic log are created in directories owned by
  you and closed to other users. Temporary files are created exclusively, so a
  planted file or symlink cannot redirect a write.
- Installing into a path with spaces or shell-special characters produces valid
  D-Bus and systemd service files.

### Removed

- The Windows taskbar widget, themes, Theme Studio, self-updater and translations.
- Google Antigravity and the Claude desktop app's login fallback, which rely on
  Windows-only credential stores.
