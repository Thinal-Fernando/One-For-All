//! `ofa.exe`, the small helper that Claude Code, Codex and the terminal call.
//!
//! Every subcommand must return quickly and exit cleanly, so it can never hold
//! up an agent.

mod client;
mod hooks;
mod process;
mod setup;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use ofa_protocol::Source;

#[derive(Parser)]
#[command(name = "ofa", version, about = "One-For-All helper")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Forward an agent hook event (JSON on stdin) to the OFA app.
    Hook {
        /// Which agent is calling.
        #[arg(long, value_enum, default_value_t = Agent::Claude)]
        agent: Agent,
    },
    /// Install or remove OFA's hooks in Claude Code's and Codex's settings.
    Setup {
        /// Remove OFA's hooks instead.
        #[arg(long)]
        uninstall: bool,
        /// Claude Code settings file to change, instead of ~/.claude/settings.json.
        #[arg(long, value_name = "FILE")]
        settings: Option<PathBuf>,
        /// Codex hooks file to change, instead of ~/.codex/hooks.json.
        #[arg(long, value_name = "FILE")]
        codex_hooks: Option<PathBuf>,
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
        Command::Hook { agent } => {
            // A failing hook shows an error in Claude Code, so problems are
            // only written to stderr, which Claude Code keeps in its debug log.
            if let Err(err) = hook(agent) {
                eprintln!("ofa hook: {err}");
            }
            ExitCode::SUCCESS
        }
        Command::Setup {
            uninstall,
            settings,
            codex_hooks,
        } => match run_setup(uninstall, settings, codex_hooks) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("ofa setup: {err}");
                ExitCode::FAILURE
            }
        },
    }
}

/// The agents `ofa hook` understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Agent {
    Claude,
    Codex,
}

impl Agent {
    fn source(self) -> Source {
        match self {
            Agent::Claude => Source::ClaudeCode,
            Agent::Codex => Source::Codex,
        }
    }

    /// The agent's own program, found by walking up from `ofa hook`.
    fn exe(self) -> &'static str {
        match self {
            Agent::Claude => "claude.exe",
            Agent::Codex => "codex.exe",
        }
    }
}

fn run_setup(
    uninstall: bool,
    claude_settings: Option<PathBuf>,
    codex_hooks: Option<PathBuf>,
) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| format!("couldn't find ofa.exe: {err}"))?;
    let targets = [
        (setup::Target::Claude, claude_settings),
        (setup::Target::Codex, codex_hooks),
    ];
    let mut failed = false;
    for (target, path) in targets {
        let Some(path) = path.or_else(|| target.default_path()) else {
            println!(
                "{}: couldn't find its settings folder, skipped",
                target.name()
            );
            continue;
        };
        // Only set up agents that are installed: their folder exists.
        if !path.parent().is_some_and(Path::exists) {
            println!("{}: not installed, skipped", target.name());
            continue;
        }
        match setup::run(target, &path, &exe, uninstall) {
            Ok(done) => {
                let what = if uninstall {
                    "removed from"
                } else {
                    "added to"
                };
                if done.changed {
                    println!(
                        "{}: OFA's hooks were {what} {}",
                        target.name(),
                        path.display()
                    );
                } else {
                    println!("{}: nothing to change in {}", target.name(), path.display());
                }
                if let Some(backup) = done.backup {
                    println!("  backup of the old file: {}", backup.display());
                }
            }
            Err(err) => {
                eprintln!("{}: {err}", target.name());
                failed = true;
            }
        }
    }
    if !uninstall {
        println!("Hooks run {}", exe.display());
        println!("New agent sessions will show on the island.");
        println!("In Codex, type /hooks once and trust OFA's hooks, or Codex won't run them.");
    }
    if failed {
        Err("some agents were not set up".into())
    } else {
        Ok(())
    }
}

/// Reads one Claude Code hook event from stdin and forwards it to the app.
fn hook(agent: Agent) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("could not read stdin: {err}"))?;
    // Windows PowerShell puts a byte order mark in front of piped text.
    let input = input.trim_start_matches('\u{feff}');
    let input: hooks::HookInput =
        serde_json::from_str(input).map_err(|err| format!("unexpected hook input: {err}"))?;
    let is_permission_prompt = input.hook_event_name == "PermissionRequest";
    let pid = process::agent_pid(agent.exe());
    let Some(event) = hooks::to_event(input, agent.source(), pid) else {
        return Ok(());
    };
    if !is_permission_prompt {
        return client::send(&event);
    }

    // Wait for an answer on the island while the terminal shows its own
    // prompt. Printing nothing leaves the decision to the terminal.
    if let Some(decision) = client::ask(&event)? {
        println!("{}", hooks::permission_reply(decision));
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
    fn hook_defaults_to_claude() {
        let cli = Cli::try_parse_from(["ofa", "hook"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Hook {
                agent: Agent::Claude
            }
        ));
        let cli = Cli::try_parse_from(["ofa", "hook", "--agent", "codex"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Hook {
                agent: Agent::Codex
            }
        ));
    }
}
