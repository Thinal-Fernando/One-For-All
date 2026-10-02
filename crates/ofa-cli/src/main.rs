//! `ofa.exe`, the small helper that Claude Code, Codex and the terminal call.
//!
//! Every subcommand must return quickly and exit cleanly, so it can never hold
//! up an agent.

mod claude;
mod client;
mod process;

use std::io::Read;
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
        #[arg(long)]
        uninstall: bool,
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
        Command::Run { .. } | Command::Setup { .. } => {
            eprintln!("ofa: not implemented yet");
            ExitCode::from(2)
        }
    }
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
    let Some(event) = claude::to_event(input, process::agent_pid("claude.exe")) else {
        return Ok(());
    };
    client::send(&event)
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
