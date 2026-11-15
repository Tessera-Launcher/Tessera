//! Outlands installation (Method B: metadata unpack).
//!
//! Outlands has no Forge-style installer to run: its `install_profile.json`
//! declares `processors: []` and no data, because the binary patches ship inside
//! the loader jar and are applied by the loader at runtime. What the artifact
//! does carry is a complete launcher profile, so installation is "read one entry
//! out of the installer jar, write it out under the isolated id".
//!
//! Unlike Fabric's profile, this one is not a delta: it has no `inheritsFrom`,
//! lists the vanilla libraries itself (with digests), and names the client jar.
//! Nothing is merged with a vanilla profile, and no hash has to be looked up.

use crate::customize::Downloader;
use crate::error::MdtError;

use super::version_json::LOADER_MARKER;
use super::{
    loader_version_id, target_version_dir, InstallMethod, InstallRequest, InstalledVersion,
};

/// Fetch the installer jar, read `version.json` out of it and write the profile.
pub async fn install(
    downloader: &dyn Downloader,
    req: InstallRequest,
) -> Result<InstalledVersion, MdtError> {
    req.reporter
        .status(&format!("获取 Outlands {} 的配置…", req.loader_version));
    let url = crate::outlands::installer_jar_url(&req.loader_version);
    let bytes = downloader.fetch_bytes(&url).await?;
    req.reporter.transferred(bytes.len() as u64);
    let text = crate::outlands::profile_from_installer(&bytes)?;

    let mut profile: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        MdtError::JsonParseError(e).context(format!("parsing Outlands profile from {url}"))
    })?;
    let Some(object) = profile.as_object_mut() else {
        return Err(MdtError::InvalidManifest(
            "Outlands profile is not a JSON object".to_string(),
        ));
    };
    if !object.contains_key("libraries") {
        return Err(MdtError::InvalidManifest(
            "Outlands profile is missing a `libraries` array".to_string(),
        ));
    }

    // The isolation convention: the directory, the JSON file name and the
    // profile's own `id` all say `1.12.2-outlands-<build>`, and vanilla's
    // `versions/1.12.2` is never touched. The profile ships with Outlands' own
    // id (`1.12.2-Outlands-2026.11.14`), which is not a directory name.
    let id = loader_version_id(&req.mc_version, req.loader, &req.loader_version);
    object.insert("id".into(), serde_json::Value::String(id.clone()));
    // Not a delta: drop any parent reference so no consumer follows one.
    object.remove("inheritsFrom");
    // Self-describing marker: without `inheritsFrom`, this is the field that
    // tells a launcher "loader install" rather than "vanilla with extras".
    object.insert(
        LOADER_MARKER.into(),
        serde_json::json!({ "id": req.loader.display_name().to_lowercase(), "version": req.loader_version }),
    );

    let vdir = target_version_dir(&req, &id);
    crate::io::create_dir_all(&vdir).await?;
    let json_path = vdir.join(format!("{id}.json"));
    crate::io::write(&json_path, serde_json::to_string_pretty(&profile)?).await?;
    Ok(InstalledVersion {
        id,
        method: InstallMethod::ManualUnpack,
        version_json: json_path,
        jar: None,
    })
}
