//! `ofa.exe`, the small helper that Claude Code, Codex and the terminal call.
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
    /// Run a command and report its exit code and duration to the OFA app.
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
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
            // A failing hook shows an error in Claude Code, so problems are
            // only written to stderr, which Claude Code keeps in its debug log.
            if let Err(err) = hook() {
                eprintln!("ofa hook: {err}");
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
        Command::Run { .. } => {
            eprintln!("ofa: not implemented yet");
            ExitCode::from(2)
        }
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

    #[test]
    fn run_keeps_the_wrapped_command_and_its_flags() {
        let cli = Cli::try_parse_from(["ofa", "run", "npm", "test", "--", "--watch"]).unwrap();
        let Command::Run { command } = cli.command else {
            panic!("expected run");
        };
        assert_eq!(command, ["npm", "test", "--", "--watch"]);
    }

    #[test]
    fn run_needs_a_command() {
        assert!(Cli::try_parse_from(["ofa", "run"]).is_err());
    }
}
