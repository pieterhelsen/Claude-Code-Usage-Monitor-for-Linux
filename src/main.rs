mod accounts;
mod app_settings;
mod daemon;
mod dbus;
mod diagnose;
#[cfg(test)]
mod https_test;
mod models;
mod poller;
mod providers;
mod snapshot;

const HELP: &str = "\
claude-code-usage-monitor — usage limits for Claude Code, Codex, Cursor, OpenCode Go and Grok

USAGE:
    claude-code-usage-monitor <MODE> [--diagnose] [--diagnose-append]

MODES:
    --daemon     Run the background service on the session D-Bus
                 (normally started by D-Bus activation or systemd)
    --json       Print the usage snapshot as JSON
    --waybar     Print a line for a waybar custom module (return-type json)
    --refresh    Ask the running daemon to poll now
    --quit       Stop the running daemon
    --version    Print the version
    --help       Show this help

    --json and --waybar read from the daemon (starting it through D-Bus
    activation when installed). Add --local to poll directly instead.

OPTIONS:
    --diagnose          Record a diagnostic log (see --help for its location)
    --diagnose-append   Append to the diagnostic log instead of replacing it

FILES:
    ~/.config/claude-code-usage-monitor/settings.json
    ~/.cache/claude-code-usage-monitor/usage-cache.json
    ~/.cache/claude-code-usage-monitor/diagnose.log
";

fn main() {
    diagnose::install_panic_hook();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|arg| arg == flag);

    if has("--diagnose") || has("--diagnose-append") {
        let result = if has("--diagnose-append") {
            diagnose::init_append()
        } else {
            diagnose::init()
        };
        match result {
            Ok(path) => {
                diagnose::log(format!("startup args={args:?} log_path={}", path.display()));
                if has("--daemon") {
                    diagnose::mirror_to_stderr(true);
                }
            }
            Err(error) => eprintln!("claude-code-usage-monitor: {error}"),
        }
    }

    let code = if has("--version") || has("-V") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        0
    } else if has("--daemon") {
        daemon::run()
    } else if has("--json") {
        print_snapshot(has("--local"), |snapshot| {
            serde_json::to_string_pretty(snapshot).unwrap_or_default()
        })
    } else if has("--waybar") {
        print_snapshot(has("--local"), |snapshot| {
            serde_json::to_string(&snapshot::waybar(snapshot, app_settings::now_unix()))
                .unwrap_or_default()
        })
    } else if has("--refresh") {
        simple_call("Refresh")
    } else if has("--quit") {
        simple_call("Quit")
    } else {
        print!("{HELP}");
        if args.is_empty() || has("--help") || has("-h") {
            0
        } else {
            2
        }
    };
    std::process::exit(code);
}

/// Prefer the daemon's snapshot so frequent callers (waybar polls every few
/// seconds) never multiply provider requests. Fall back to a local poll.
fn print_snapshot(local: bool, render: impl Fn(&snapshot::Snapshot) -> String) -> i32 {
    if !local {
        match dbus::call("GetUsage") {
            Ok(Some(json)) => match serde_json::from_str::<snapshot::Snapshot>(&json) {
                Ok(snapshot) => {
                    println!("{}", render(&snapshot));
                    return 0;
                }
                Err(error) => diagnose::log_error("daemon returned invalid JSON", error),
            },
            Ok(None) => {}
            Err(error) => diagnose::log_error("daemon unavailable; polling locally", error),
        }
    }
    let settings = app_settings::load_settings();
    let previous = app_settings::load_usage_cache()
        .map(|cache| cache.data)
        .unwrap_or_default();
    let (usage, ok) = daemon::poll_once(&settings, &previous, false, |_| {});
    let _ = app_settings::save_usage_cache(&usage, ok);
    let now = app_settings::now_unix();
    let snapshot = snapshot::Snapshot::build(
        &settings,
        &usage,
        snapshot::Runtime {
            fetched_at: now,
            next_poll_at: 0,
            polling: false,
            poll_ok: ok,
        },
    );
    println!("{}", render(&snapshot));
    if ok {
        0
    } else {
        1
    }
}

fn simple_call(method: &str) -> i32 {
    match dbus::call(method) {
        Ok(_) => 0,
        Err(error) => {
            eprintln!("claude-code-usage-monitor: cannot reach the daemon: {error}");
            1
        }
    }
}
