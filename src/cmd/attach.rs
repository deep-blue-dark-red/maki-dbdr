//! `maki attach <session>`: watch a session live in a read-only TUI. The
//! process that owns the session keeps running untouched.

use std::str::FromStr;

use color_eyre::Result;
use color_eyre::eyre::{Context, bail, eyre};
use maki_agent::session::StoredSession;
use maki_config::project::TrustMode;
use maki_storage::StateDir;
use maki_storage::id::SessionRef;
use maki_ui::attach::AttachParams;

/// The id is echoed back the way the user spelled it, so a partial or
/// non-canonical form they passed stays recognizable in the error.
const NO_SESSION: &str = "no session {raw} found; pick an id from `maki session list`";
const BAD_REF: &str = "'{raw}' is not a session id";

pub fn run(session: &str, no_plugins: bool, no_jit: bool, trust_mode: TrustMode) -> Result<()> {
    let id = SessionRef::from_str(session)
        .map_err(|e| eyre!("{}: {e}", BAD_REF.replace("{raw}", session)))?
        .id();
    let storage = StateDir::resolve().context("resolve data directory")?;
    if !StoredSession::exists(id, &storage) {
        bail!(NO_SESSION.replace("{raw}", session));
    }
    let (_host, config) = super::cli_stack(no_plugins, no_jit, trust_mode)?;

    // The theme last picked in this folder outranks the config knob, same as
    // the TUI's boot.
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| ".".into())
        .to_string_lossy()
        .into_owned();
    let theme_name =
        maki_storage::sessions::cwd_theme(&storage, &cwd).or_else(|| config.ui.theme.clone());

    maki_ui::attach::run(AttachParams {
        id,
        storage,
        ui_config: config.ui.clone(),
        theme_name,
    })
}
