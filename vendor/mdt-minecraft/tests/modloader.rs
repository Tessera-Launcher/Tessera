//! Tests for Fabric version IDs, mirror rewriting, and version parsing.

#[cfg(not(feature = "minimal"))]
use mdt::installer::{
    apply_mirror_to_json, installer_version_id, library_path_from_name, loader_version_id,
    maven_artifact_path, GeneratedLibrary, GeneratedVersionJson,
};
use mdt::manifest::{
    loader_versions_from_data, FabricVersionsList, LatestMinecraftVersion, ManifestData,
    MinecraftVersionsList,
};
use mdt::{ManifestUrls, McVersion, Mirror, ModLoader};

#[test]
fn test_loader_compatible_with() {
    // Fabric: 1.14 and later.
    assert!(!ModLoader::Fabric.compatible_with("1.12.2"));
    assert!(ModLoader::Fabric.compatible_with("1.14"));
    assert!(ModLoader::Fabric.compatible_with("1.20.1"));

    // New-style year numbering (26.x onward): treated as modern.
    assert!(ModLoader::Fabric.compatible_with("26.3"));
}

#[test]
fn test_mc_version_parse() {
    // Old style.
    assert_eq!(McVersion::parse("1.12.2"), Some(McVersion::new(1, 12, 2)));
    assert_eq!(McVersion::parse("1.14"), Some(McVersion::new(1, 14, 0)));
    assert_eq!(McVersion::parse("1.21.1"), Some(McVersion::new(1, 21, 1)));
    // New-style year numbering (26.x onward).
    assert_eq!(McVersion::parse("26.3"), Some(McVersion::new(26, 3, 0)));
    assert_eq!(McVersion::parse("26.3.1"), Some(McVersion::new(26, 3, 1)));
    // Suffix stripping.
    assert_eq!(McVersion::parse("1.21-rc1"), Some(McVersion::new(1, 21, 0)));
    assert_eq!(
        McVersion::parse("26.3-rc-3"),
        Some(McVersion::new(26, 3, 0))
    );
    // Dotless snapshots are unparseable.
    assert_eq!(McVersion::parse("24w05a"), None);
    // New style > any old style.
    assert!(McVersion::parse("26.3").unwrap() > McVersion::parse("1.21.1").unwrap());
}

#[cfg(not(feature = "minimal"))]
#[test]
fn test_loader_version_id() {
    assert_eq!(
        loader_version_id("1.16.5", ModLoader::Fabric, "0.20.0"),
        "1.16.5-fabric-0.20.0"
    );
}

#[cfg(not(feature = "minimal"))]
#[test]
fn test_installer_version_id() {
    assert_eq!(
        installer_version_id("1.20.1", ModLoader::Fabric, "0.15.11"),
        "1.20.1-fabric-0.15.11"
    );
}

#[cfg(not(feature = "minimal"))]
#[test]
fn test_maven_artifact_path() {
    assert_eq!(
        maven_artifact_path(
            "net.fabricmc",
            "fabric-loader",
            "0.15.11",
            Some("installer")
        ),
        "net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11-installer.jar"
    );
}

#[cfg(not(feature = "minimal"))]
#[test]
fn test_library_path_from_name() {
    assert_eq!(
        library_path_from_name("net.fabricmc:fabric-loader:0.15.11").unwrap(),
        "net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar"
    );
    assert!(library_path_from_name("bad").is_err());
    assert!(library_path_from_name("a:b").is_err());
}

#[test]
fn test_apply_maven_mirror() {
    let urls = ManifestUrls::new(&Mirror::BMCLAPI);
    const M: &str = "https://bmclapi2.bangbang93.com/maven";

    assert_eq!(
        urls.apply_maven_mirror("https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar", &Mirror::BMCLAPI),
        format!("{M}/net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar")
    );
    // Mojang source returns unchanged.
    let mojang = ManifestUrls::new(&Mirror::Mojang);
    assert_eq!(
        mojang.apply_maven_mirror("https://maven.fabricmc.net/x", &Mirror::Mojang),
        "https://maven.fabricmc.net/x"
    );
}

#[cfg(not(feature = "minimal"))]
#[test]
fn test_generated_library_maven() {
    let lib = GeneratedLibrary::maven(
        "net.fabricmc:fabric-loader:0.15.11",
        "https://maven.fabricmc.net/",
    )
    .unwrap();
    assert_eq!(lib.name, "net.fabricmc:fabric-loader:0.15.11");
    assert_eq!(
        lib.downloads.artifact.path,
        "net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar"
    );
    assert_eq!(
        lib.downloads.artifact.url,
        "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar"
    );
}

#[test]
fn test_profile_url_order() {
    let mojang = ManifestUrls::new(&Mirror::Mojang);
    assert_eq!(
        mojang.fabric_profile("0.16.14", "1.20.1"),
        "https://meta.fabricmc.net/v2/versions/loader/1.20.1/0.16.14/profile/json"
    );
}

#[test]
fn test_loader_versions_come_from_the_fabric_manifest() {
    let data = ManifestData {
        minecraft: MinecraftVersionsList {
            latest: LatestMinecraftVersion {
                release: String::new(),
                snapshot: String::new(),
            },
            versions: vec![],
        },
        fabric: FabricVersionsList {
            loader: vec![
                mdt::LoaderVersionEntry {
                    version: "0.16.14".to_string(),
                    stable: true,
                },
                mdt::LoaderVersionEntry {
                    version: "0.16.15-beta.1".to_string(),
                    stable: false,
                },
            ],
        },
    };

    let versions = loader_versions_from_data(&data);
    assert_eq!(versions, vec!["0.16.14", "0.16.15-beta.1"]);
}

#[cfg(not(feature = "minimal"))]
#[test]
fn test_apply_mirror_to_json() {
    let urls = ManifestUrls::new(&Mirror::BMCLAPI);
    let profile: serde_json::Value = serde_json::from_str(
        r#"{
            "id": "fabric-loader-0.15.11-1.20.1",
            "libraries": [
                {"name": "net.fabricmc:intermediary:1.20.1", "url": "https://maven.fabricmc.net/"}
            ]
        }"#,
    )
    .unwrap();

    let mirrored = apply_mirror_to_json(profile, &Mirror::BMCLAPI, &urls);
    let libs = mirrored["libraries"].as_array().unwrap();
    assert_eq!(libs[0]["url"], "https://bmclapi2.bangbang93.com/maven/");
}

#[test]
fn test_provider_name_resolves_builtin_name() {
    assert_eq!(
        Mirror::Custom("mojang".to_string()).provider_name(),
        "mojang"
    );
    assert_eq!(
        Mirror::Custom("bmclapi".to_string()).provider_name(),
        "bmclapi"
    );

    // A custom mirror keyed to a built-in provider name resolves to the same endpoint table.
    let via_custom = ManifestUrls::new(&Mirror::Custom("bmclapi".to_string()));
    let via_builtin = ManifestUrls::new(&Mirror::BMCLAPI);
    assert_eq!(via_custom.minecraft, via_builtin.minecraft);
    assert_eq!(via_custom.fabric_maven, via_builtin.fabric_maven);

    // `other()` on a custom mirror falls back to the default mirror.
    assert_eq!(
        Mirror::Custom("anything".to_string()).other(),
        Mirror::default()
    );
}

#[cfg(not(feature = "minimal"))]
#[test]
fn test_generated_version_json_serialization() {
    let json = GeneratedVersionJson {
        id: "1.20.1-fabric-0.15.11".to_string(),
        inherits_from: Some("1.20.1".to_string()),
        time: Some("2024-01-01T00:00:00Z".to_string()),
        release_time: Some("2024-01-01T00:00:00Z".to_string()),
        version_type: "release".to_string(),
        main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".to_string(),
        minecraft_arguments: None,
        libraries: vec![GeneratedLibrary::maven(
            "net.fabricmc:fabric-loader:0.15.11",
            "https://maven.fabricmc.net/",
        )
        .unwrap()],
    };

    let s = json.to_pretty_json().unwrap();
    assert!(s.contains("\"inheritsFrom\": \"1.20.1\""));
    assert!(s.contains("\"mainClass\": \"net.fabricmc.loader.impl.launch.knot.KnotClient\""));
    assert!(s.contains("net.fabricmc:fabric-loader:0.15.11"));
}
