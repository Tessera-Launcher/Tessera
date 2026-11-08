//! The 26.x menu-fade agent.
//!
//! Minecraft 26.x shows a one-second LoadingOverlay fade and a two-second
//! TitleScreen widget fade after the resources are already loaded. Neither
//! does any work, and the window looks "slow" while they run. The agent in
//! `assets/fastmenu-agent.jar` moves the private fade timestamps into the past
//! through reflection, so the title screen appears at full opacity.
//!
//! It is injected for every 26.x profile, with or without a Mod Loader: the
//! class names are unobfuscated in that series. The agent is deliberately
//! reflective and swallows failures, so a changed field means "no fast menu",
//! never a failed launch.

use std::fs;
use std::path::Path;

/// The compiled agent; source is next to it in `assets/fastmenu-agent.java`.
pub const AGENT: &[u8] = include_bytes!("../assets/fastmenu-agent.jar");

/// Whether this profile id is in the unobfuscated 26.x series.
pub fn eligible(version: &str) -> bool {
    version
        .strip_prefix("26.")
        .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()))
}

/// Copy the agent next to the launcher's cache, if it is not there yet.
pub fn ensure(path: &Path) -> std::io::Result<()> {
    if fs::read(path).is_ok_and(|existing| existing == AGENT) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, AGENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_covers_26_x_but_not_1_x() {
        for version in [
            "26.0",
            "26.1",
            "26.3",
            "26.4-snapshot-7",
            "26.3+Fabric0.20.1",
        ] {
            assert!(eligible(version), "{version}");
        }
        for version in ["1.21.11", "25w46a", "27.0", "b1.8.1"] {
            assert!(!eligible(version), "{version}");
        }
    }
}
