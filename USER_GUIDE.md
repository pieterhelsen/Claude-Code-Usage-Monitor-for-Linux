# User guide

This guide covers everyday settings. For installation, see the [README](README.md).

## Open the settings

Click the indicator in the top bar and choose **Settings**. You can also run:

```sh
gnome-extensions prefs claude-code-usage-monitor@pieterhelsen.github.io
```

The window has two pages:

- **Panel** controls what the top bar shows. These settings belong to the extension.
- **Providers** controls what the daemon collects. These settings are shared
  with waybar and every other front end.

## Choose what the top bar shows

On the **Panel** page:

- **Provider** picks whose usage appears. *Automatic* uses the first enabled
  provider that has data, in the order Claude Code, Codex, OpenCode, Cursor, Grok.
- **Usage window** picks which figure appears. *Closest to its limit* shows
  whichever window is fullest. When paid credits are in use, it shows those.
  You can also pin the label to the 5-hour, weekly, monthly or credits window.
- **Icon**, **Percentage**, **Time until reset** and **Usage bar** switch the
  parts of the label on and off.
- **Amber from** and **Red from** set the warning colours. They always compare
  against usage spent, even when the label shows what is left.
- **Placement** moves the indicator to the left, centre or right of the top bar.

## Show used or remaining allowance

On the **Providers** page, **Show what is left** switches every front end between
usage spent (`42%`) and allowance remaining (`58%`). A fresh limit then reads 100%
and drains as you work.

## Choose providers and refresh

On the **Providers** page, switch providers on or off. The subtitle under each
provider shows the latest error, if there is one. At least one provider always
stays enabled.

**Refresh every** sets how often the daemon polls. The daemon also polls right
after a usage window resets and within 30 seconds of a login file changing.
**Refresh now** polls immediately; so does **Refresh now** in the top-bar menu.

## Several Claude Code or Codex accounts

The daemon can watch several logins at once, for example a work and a personal
account. Add them to the settings file, `~/.config/claude-code-usage-monitor/settings.json`:

```json
{
  "accounts": {
    "claude": {
      "profiles": [
        {"id": "default", "name": "Personal", "enabled": true},
        {"id": "account_1", "name": "Work", "config_dir": "~/.claude-work", "enabled": true}
      ],
      "selected": "default"
    }
  }
}
```

- `config_dir` points to that login's `CLAUDE_CONFIG_DIR` (or `CODEX_HOME` for Codex).
- `credentials_path` can point at a credentials file directly instead.
- `selected` picks which account drives the top bar. The others appear under
  **Other accounts** in the menu.

After editing the file, restart the daemon with `claude-code-usage-monitor --quit`;
D-Bus starts it again on next use.

## Files

| Path | Contents |
| --- | --- |
| `~/.config/claude-code-usage-monitor/settings.json` | Daemon settings |
| `~/.cache/claude-code-usage-monitor/usage-cache.json` | Last usage reading, shown until the next poll |
| `~/.cache/claude-code-usage-monitor/diagnose.log` | Diagnostic log, when enabled |

Set `CLAUDE_CODE_USAGE_MONITOR_CONFIG_DIR` to keep all of these under one other
directory, for example to try a build without touching your real settings.
