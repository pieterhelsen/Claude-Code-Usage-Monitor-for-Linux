//! Locating and driving provider CLIs (claude, codex, grok) on Linux.
//!
//! The daemon usually runs under systemd or D-Bus activation, where `PATH` is
//! the minimal session path and misses per-user install locations such as
//! `~/.local/bin` or npm's global prefix. Those are searched as well.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Per-user directories CLIs are commonly installed into, relative to `$HOME`.
const HOME_BIN_DIRECTORIES: &[&str] = &[
    ".local/bin",
    ".claude/local",
    ".npm-global/bin",
    ".bun/bin",
    ".cargo/bin",
    ".grok/bin",
    ".volta/bin",
];

pub fn find_executable(name: &str) -> Option<PathBuf> {
    find_executable_in(&search_directories(), name)
}

fn search_directories() -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        directories.extend(HOME_BIN_DIRECTORIES.iter().map(|dir| home.join(dir)));
    }
    directories
}

pub fn find_executable_in(directories: &[PathBuf], name: &str) -> Option<PathBuf> {
    directories
        .iter()
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// A detached, silent invocation that cannot be mistaken for a nested
/// Claude Code session. It runs in an empty private directory, so an agent
/// CLI started only to renew its login never sees a project or the home
/// directory as its workspace.
pub fn command(path: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(path);
    if let Some(directory) = scratch_directory() {
        command.current_dir(directory);
    }
    command
        .args(args)
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// `~/.cache/claude-code-usage-monitor/cli`, emptied before each use.
pub fn scratch_directory() -> Option<PathBuf> {
    let directory = crate::app_settings::cache_directory().join("cli");
    let _ = std::fs::remove_dir_all(&directory);
    crate::app_settings::create_private_dir(&directory).ok()?;
    Some(directory)
}

/// Run to completion and capture output, killing the process on timeout.
#[cfg(test)]
pub fn run_with_timeout(command: &mut Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = command.spawn().ok()?;
    if wait_for(&mut child, timeout) {
        child.wait_with_output().ok()
    } else {
        None
    }
}

/// Wait for a child to exit. Returns false (after killing it) on timeout.
pub fn wait_for(child: &mut Child, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
pub fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_executable_files_in_order() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::write(first.path().join("tool"), "not executable").unwrap();
        write_script(&second.path().join("tool"), "exit 0");
        let directories = vec![first.path().to_path_buf(), second.path().to_path_buf()];
        assert_eq!(
            find_executable_in(&directories, "tool"),
            Some(second.path().join("tool"))
        );
        assert_eq!(find_executable_in(&directories, "missing"), None);
    }

    #[test]
    fn relative_path_entries_are_ignored() {
        assert_eq!(
            find_executable_in(&[PathBuf::from(".")], "Cargo.toml"),
            None
        );
    }

    #[test]
    fn output_is_captured_and_slow_commands_are_killed() {
        let directory = tempfile::tempdir().unwrap();
        let fast = directory.path().join("fast");
        let slow = directory.path().join("slow");
        write_script(&fast, "echo hello");
        write_script(&slow, "sleep 30");
        let output = run_with_timeout(
            Command::new(&fast).stdout(Stdio::piped()),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
        let start = Instant::now();
        assert!(run_with_timeout(&mut Command::new(&slow), Duration::from_millis(300)).is_none());
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
