//! `ofa.exe`, the small helper that Claude Code, Codex and the terminal call.
//!
//! Every subcommand must return quickly and exit cleanly, so it can never hold
//! up an agent. In milestone 1 the subcommands are placeholders.

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
        // A failing hook can interrupt the agent, so this placeholder always succeeds.
        Command::Hook => ExitCode::SUCCESS,
        Command::Run { .. } | Command::Setup { .. } => {
            eprintln!("ofa: not implemented yet");
            ExitCode::from(2)
        }
    }
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
