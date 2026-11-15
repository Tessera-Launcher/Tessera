//! Supported mod loaders.

use crate::mc_version::McVersion;

/// A supported Minecraft mod loader.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModLoader {
    /// Lightweight, high-performance loader for 1.14+.
    Fabric,
    /// 1.12.2 loader forked from Cleanroom/Forge, on Java 25 + LWJGL 3.
    Outlands,
}

impl ModLoader {
    /// The canonical display name, used in isolated version IDs and directory names
    /// (e.g. `1.20.1-fabric-0.15.11`).
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            ModLoader::Fabric => "Fabric",
            ModLoader::Outlands => "Outlands",
        }
    }

    /// Whether this loader supports the given Minecraft version.
    ///
    /// Fabric supports `1.14` and later, and unparseable versions (e.g. weekly
    /// snapshots like `24w05a`) are treated as modern versions. Outlands is a
    /// continuation of the 1.12.2 stack and exists for that version only, so a
    /// snapshot is never a match.
    #[must_use]
    pub fn compatible_with(self, mc_version: &str) -> bool {
        match self {
            ModLoader::Outlands => mc_version == "1.12.2",
            ModLoader::Fabric => {
                let Some(v) = McVersion::parse(mc_version) else {
                    return true;
                };
                v >= McVersion::new(1, 14, 0)
            }
        }
    }
}
