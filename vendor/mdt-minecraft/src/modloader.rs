//! Supported mod loaders.

use crate::mc_version::McVersion;

/// A supported Minecraft mod loader.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModLoader {
    /// Lightweight, high-performance loader for 1.14+.
    Fabric,
}

impl ModLoader {
    /// The canonical display name, used in isolated version IDs and directory names
    /// (e.g. `1.20.1-fabric-0.15.11`).
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            ModLoader::Fabric => "Fabric",
        }
    }

    /// Whether this loader supports the given Minecraft version.
    ///
    /// Fabric supports `1.14` and later. Unparseable versions (e.g. weekly
    /// snapshots like `24w05a`) are treated as modern versions.
    #[must_use]
    pub fn compatible_with(self, mc_version: &str) -> bool {
        let Some(v) = McVersion::parse(mc_version) else {
            return true;
        };
        match self {
            ModLoader::Fabric => v >= McVersion::new(1, 14, 0),
        }
    }
}
