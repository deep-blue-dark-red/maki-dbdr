mod acp;
mod migrate;
mod subcmd;
mod tui;

use color_eyre::Result;
use color_eyre::eyre::Context;

use maki_storage::StateDir;

use crate::cli::{AuthAction, Cli, Command, McpAction, MigrateAction};
use crate::update;

pub fn dispatch(cli: Cli) -> Result<()> {
    // `/system_prompt` edits <config_dir>/system.md; pick it up before any
    // command builds a prompt. A broken file shouldn't stop the CLI booting,
    // so warn and fall back to the built-in prompt.
    if let Err(e) = maki_agent::prompt::load_user_system_prompt() {
        eprintln!("warning: could not read custom system prompt: {e}");
    }

    match cli.command {
        Some(Command::Auth { action }) => {
            let storage = StateDir::resolve().context("resolve data directory")?;
            match action {
                AuthAction::Login { provider } => {
                    subcmd::auth_login(provider.as_deref(), &storage)?
                }
                AuthAction::Logout { provider } => subcmd::auth_logout(&provider, &storage)?,
                AuthAction::Status => subcmd::auth_status(&storage)?,
            }
        }
        Some(Command::Index { path }) => {
            subcmd::index(&path, cli.no_plugins, cli.no_jit)?;
        }
        Some(Command::Models) => subcmd::models(cli.no_plugins, cli.no_jit)?,
        Some(Command::Mcp { action }) => {
            let storage = StateDir::resolve().context("resolve data directory")?;
            match action {
                McpAction::Auth { server } => subcmd::mcp_auth(&server, &storage)?,
                McpAction::Logout { server } => subcmd::mcp_logout(&server, &storage)?,
            }
        }
        Some(Command::Update { yes, no_color }) => {
            update::update(yes, no_color).map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
        }
        Some(Command::Rollback) => {
            update::rollback().map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
        }
        Some(Command::Acp { model, yolo }) => {
            acp::run(model, yolo, cli.no_plugins, cli.no_jit)?;
        }
        Some(Command::Migrate { action }) => match action {
            MigrateAction::Xdg => migrate::xdg()?,
        },
        Some(Command::Prompt {
            variant,
            plan,
            tools,
            names,
        }) => {
            subcmd::prompt(&variant, plan, tools, names, cli.no_plugins, cli.no_jit)?;
        }
        None => {
            tui::run(cli)?;
        }
    }
    Ok(())
}
