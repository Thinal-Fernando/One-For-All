//! `ofa.exe`, the small helper that Claude Code calls through its hooks.
//!
//! Every subcommand must return quickly and exit cleanly, so it can never hold
//! up an agent.

mod claude;
mod client;
mod process;
mod setup;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ofa", version, about = "One-For-All helper")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Forward an agent hook event (JSON on stdin) to the OFA app.
    Hook,
    /// Install or remove OFA's hooks in Claude Code's settings.
    Setup {
        /// Remove OFA's hooks instead.
        #[arg(long)]
        uninstall: bool,
        /// Settings file to change, instead of ~/.claude/settings.json.
        #[arg(long, value_name = "FILE")]
        settings: Option<PathBuf>,
    },
    /// Show the helper's version and where it expects the OFA app.
    Status,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Status => {
            println!(
                "ofa {} (protocol v{}), app API at http://{}",
                env!("CARGO_PKG_VERSION"),
                ofa_protocol::PROTOCOL_VERSION,
                ofa_protocol::api_addr()
            );
            ExitCode::SUCCESS
        }
        Command::Hook => {
            // A failing hook shows an error in Claude Code, so problems only
            // go to stderr and to OFA's own log, never to the exit code.
            if let Err(err) = hook() {
                eprintln!("ofa hook: {err}");
                log_hook_error(&err);
            }
            ExitCode::SUCCESS
        }
        Command::Setup {
            uninstall,
            settings,
        } => match run_setup(uninstall, settings) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("ofa setup: {err}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Largest the hook error log grows before it starts over.
const MAX_LOG_BYTES: u64 = 64 * 1024;

/// Appends one line to `%LOCALAPPDATA%\OFA\hook-errors.log`. Claude Code
/// hides a hook's stderr, so without this a failing hook is invisible. The
/// log is capped, so a closed island adding a line per hook does no harm.
fn log_hook_error(err: &str) {
    let Some(dir) = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("OFA")) else {
        return;
    };
    let path = dir.join("hook-errors.log");
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES);
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let user = std::env::var("USERNAME").unwrap_or_default();
    let line = format!("{seconds} user={user}: {err}\n");
    let _ = std::fs::create_dir_all(&dir);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(true)
        .truncate(too_big)
        .open(&path);
    if let Ok(mut file) = file {
        let _ = std::io::Write::write_all(&mut file, line.as_bytes());
    }
}

fn run_setup(uninstall: bool, settings: Option<PathBuf>) -> Result<(), String> {
    let path = settings
        .or_else(setup::settings_path)
        .ok_or("couldn't find your Claude Code settings folder")?;
    let exe = std::env::current_exe().map_err(|err| format!("couldn't find ofa.exe: {err}"))?;
    let done = setup::run(&path, &exe, uninstall)?;

    let what = if uninstall {
        "removed from"
    } else {
        "added to"
    };
    if done.changed {
        println!("OFA's hooks were {what} {}", done.settings.display());
    } else {
        println!("Nothing to change in {}", done.settings.display());
    }
    if let Some(backup) = done.backup {
        println!("Backup of the old file: {}", backup.display());
    }
    if !uninstall {
        println!("Hooks run {}", exe.display());
        println!("New Claude Code sessions will show on the island.");
    }
    Ok(())
}

/// Reads one Claude Code hook event from stdin and forwards it to the app.
fn hook() -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("could not read stdin: {err}"))?;
    // Windows PowerShell puts a byte order mark in front of piped text.
    let input = input.trim_start_matches('\u{feff}');
    let input: claude::HookInput =
        serde_json::from_str(input).map_err(|err| format!("unexpected hook input: {err}"))?;
    let is_permission_prompt = input.hook_event_name == "PermissionRequest";
    let Some(event) = claude::to_event(input, process::agent_pid("claude.exe")) else {
        return Ok(());
    };
    if !is_permission_prompt {
        return client::send(&event);
    }

    // Wait for an answer on the island while the terminal shows its own
    // prompt. Printing nothing leaves the decision to the terminal.
    if let Some(decision) = client::ask(&event)? {
        println!("{}", claude::permission_reply(decision));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
