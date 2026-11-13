//! Tests for launch assembly: version chains, arguments, Java choice, natives
//! and AOT cache selection. Only the real-game `live_*` tests are ignored.

use std::collections::HashMap;

use super::aot_cache::aot_cache;
use super::build::is_1_8;
use super::java::parse_java_major;
use super::manifest::{
    client_jar, features, load_chain, maven_path, rules_allow, substitute, OsRule, Rule,
    MAX_CHAIN_DEPTH,
};
use super::natives::{ascii_natives_mirror, find_natives, lwjgl_needs_ascii_natives};
use super::*;

#[test]
fn offline_credentials_are_stable_and_legacy() {
    let a = Credentials::offline("Steve");
    let b = Credentials::offline("Steve");
    assert_eq!(a, b);
    assert_eq!(a.user_type, "legacy");
    assert_eq!(a.access_token, "0");
    assert_eq!(a.uuid.len(), 36);
    assert_ne!(a.uuid, Credentials::offline("Alex").uuid);
}

#[test]
fn java_acceptability_pins_java8_profiles_and_rolls_forward_newer_ones() {
    // 1.8.9 asks for 8: only 8 will do.
    assert!(java_is_acceptable(8, 8));
    assert!(!java_is_acceptable(8, 17));
    assert!(!java_is_acceptable(8, 25));
    // 1.21 asks for 21: anything newer is fine, older is not.
    assert!(java_is_acceptable(21, 21));
    assert!(java_is_acceptable(21, 25));
    assert!(!java_is_acceptable(21, 17));
}

/// A managed runtime at `<root>/运行时/<version>/bin/java`, as the picker
/// leaves it. Returns the runtimes directory.
fn managed_runtime(root: &Path, major: u32) -> PathBuf {
    let version = format!("{major}.0.1+1");
    let bin = crate::java::install_dir(&root.join("运行时"), &version).join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    fake_java(&bin, major);
    root.join("运行时")
}

#[test]
fn aot_status_explains_why_a_trained_cache_would_be_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let runtimes = managed_runtime(tmp.path(), 8);
    let version_dir = tmp.path().join("versions/1.8.9");
    let cache = version_dir.join("1.8.9.aot");
    let unsigned = version_dir.join("1.8.9.unsigned.jar");
    let agent = version_dir.join("1.8.9.aot-agent.jar");

    // Untrained: nothing to say at all.
    assert!(aot_status(&version_dir, "1.8.9", &runtimes).is_none());

    std::fs::write(&cache, b"cache").unwrap();
    std::fs::write(&unsigned, b"jar").unwrap();
    std::fs::write(&agent, b"agent").unwrap();
    std::fs::write(
        version_dir.join("1.8.9.aot.json"),
        r#"{"java_major":8,"java_version":"8.0.1+1","unsigned_jar":"1.8.9.unsigned.jar","agent":"1.8.9.aot-agent.jar"}"#,
    )
    .unwrap();
    let ready = aot_status(&version_dir, "1.8.9", &runtimes)
        .unwrap()
        .unwrap();
    assert!(ready.contains("Java 8"), "{ready}");

    std::fs::remove_file(&unsigned).unwrap();
    let broken = aot_status(&version_dir, "1.8.9", &runtimes)
        .unwrap()
        .unwrap_err();
    assert!(
        broken.to_string().contains("1.8.9.unsigned.jar"),
        "{broken}"
    );

    // A cache trained by a runtime that has since been removed is equally
    // unusable, and the log names that runtime.
    std::fs::write(&unsigned, b"jar").unwrap();
    std::fs::write(
        version_dir.join("1.8.9.aot.json"),
        r#"{"java_major":25,"java_version":"25.0.5+8","unsigned_jar":"1.8.9.unsigned.jar","agent":"1.8.9.aot-agent.jar"}"#,
    )
    .unwrap();
    let missing = aot_status(&version_dir, "1.8.9", &runtimes)
        .unwrap()
        .unwrap_err();
    assert!(missing.to_string().contains("25.0.5+8"), "{missing}");
}

#[test]
fn prepare_uses_a_downloaded_runtime_when_it_has_one() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let runtimes = managed_runtime(tmp.path(), 28);
    let java = crate::java::install_dir(&runtimes, "28.0.1+1").join("bin/java");

    let options = Options {
        runtimes: Some(runtimes),
        ..Options::default()
    };
    let launch = prepare(
        tmp.path(),
        "1.8.9",
        &Credentials::offline("Steve"),
        &options,
    )
    .unwrap();
    assert_eq!(launch.command.exe, java, "the managed runtime is used");
    assert_eq!(launch.java_major, 28);
}

#[test]
fn a_finished_training_run_pins_the_launch_to_its_own_jvm_and_cache() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let runtimes = managed_runtime(tmp.path(), 8);
    let version_dir = tmp.path().join("versions/1.8.9");
    let cache = version_dir.join("1.8.9.aot");
    let unsigned = version_dir.join("1.8.9.unsigned.jar");
    let agent = version_dir.join("1.8.9.aot-agent.jar");
    std::fs::write(&cache, b"cache").unwrap();
    std::fs::write(&unsigned, b"jar").unwrap();
    std::fs::write(&agent, b"agent").unwrap();
    std::fs::write(
        version_dir.join("1.8.9.aot.json"),
        r#"{"java_major":8,"java_version":"8.0.1+1","unsigned_jar":"1.8.9.unsigned.jar","agent":"1.8.9.aot-agent.jar"}"#,
    )
    .unwrap();

    let found = aot_cache(&version_dir, "1.8.9", &runtimes).expect("cache + metadata");
    assert_eq!(found.cache, cache);
    assert_eq!(found.java_major, 8);
    assert_eq!(found.unsigned_jar, unsigned);

    let options = Options {
        runtimes: Some(runtimes),
        ..Options::default()
    };
    let launch = prepare(
        tmp.path(),
        "1.8.9",
        &Credentials::offline("Steve"),
        &options,
    )
    .unwrap();
    assert!(
        launch
            .command
            .args
            .iter()
            .any(|arg| arg == &format!("-XX:AOTCache={}", cache.display())),
        "{:?}",
        launch.command.args
    );
    assert!(
        launch
            .command
            .args
            .iter()
            .any(|arg| arg == &format!("-javaagent:{}", agent.display())),
        "{:?}",
        launch.command.args
    );
    assert_eq!(launch.java_major, 8);
}

#[test]
fn only_managed_runtimes_are_considered() {
    let tmp = tempfile::tempdir().unwrap();
    let runtimes = tmp.path().join("运行时");
    // Nothing installed: the error says what to do, not "install Java on PATH".
    let error = find_java(&runtimes, 21, JavaKind::Loader).unwrap_err();
    assert!(error.to_string().contains("Java 运行时界面"), "{error}");
    assert!(!error.to_string().contains("JAVA_HOME"), "{error}");

    managed_runtime(tmp.path(), 8);
    managed_runtime(tmp.path(), 21);

    // Exact wins, 8 stays pinned, and a 17 profile takes the smallest newer one.
    assert_eq!(find_java(&runtimes, 21, JavaKind::Loader).unwrap().1, 21);
    assert_eq!(find_java(&runtimes, 8, JavaKind::Loader).unwrap().1, 8);
    assert_eq!(find_java(&runtimes, 17, JavaKind::Loader).unwrap().1, 21);
    // A 25 profile has nothing to run on, and the message lists what exists.
    let error = find_java(&runtimes, 25, JavaKind::Loader).unwrap_err();
    assert!(error.to_string().contains("Java 25"), "{error}");
    assert!(error.to_string().contains("Java 8"), "{error}");
    assert!(error.to_string().contains("Java 21"), "{error}");
}

#[test]
fn vanilla_locks_java_28_and_loaders_keep_their_pin() {
    let tmp = tempfile::tempdir().unwrap();
    managed_runtime(tmp.path(), 8);
    managed_runtime(tmp.path(), 17);
    managed_runtime(tmp.path(), 28);
    let runtimes = tmp.path().join("运行时");

    // Vanilla: 28 and only 28, even for a 1.8.9 profile that pins 8. That is
    // the only runtime the launch shims have been tested on.
    assert_eq!(find_java(&runtimes, 8, JavaKind::Vanilla).unwrap().1, 28);
    assert_eq!(find_java(&runtimes, 17, JavaKind::Vanilla).unwrap().1, 28);
    assert_eq!(find_java(&runtimes, 26, JavaKind::Vanilla).unwrap().1, 28);
    // A newer minimum than 28 cannot be run here, and the message says why.
    let error = find_java(&runtimes, 30, JavaKind::Vanilla).unwrap_err();
    assert!(error.to_string().contains("30"), "{error}");

    // Loaders keep the old rule: exact 8 for a legacy profile, never 28.
    assert_eq!(find_java(&runtimes, 8, JavaKind::Loader).unwrap().1, 8);
    assert_eq!(find_java(&runtimes, 17, JavaKind::Loader).unwrap().1, 17);
    assert_eq!(find_java(&runtimes, 21, JavaKind::Loader).unwrap().1, 28);
}

#[test]
fn a_vanilla_launch_without_java_28_asks_for_java_28() {
    let tmp = tempfile::tempdir().unwrap();
    let error = find_java(&tmp.path().join("运行时"), 8, JavaKind::Vanilla).unwrap_err();
    assert!(error.to_string().contains("Java 28"), "{error}");

    // A 21 on disk is not a substitute: vanilla is locked, not "newest".
    managed_runtime(tmp.path(), 8);
    managed_runtime(tmp.path(), 21);
    let error = find_java(&tmp.path().join("运行时"), 8, JavaKind::Vanilla).unwrap_err();
    assert!(error.to_string().contains("Java 28"), "{error}");
}

#[test]
fn an_applet_era_version_locks_java_28_too() {
    let tmp = tempfile::tempdir().unwrap();
    managed_runtime(tmp.path(), 8);
    managed_runtime(tmp.path(), 25);
    managed_runtime(tmp.path(), 28);
    let runtimes = tmp.path().join("运行时");

    // Up to 1.5.2 the game is an applet, but the launch shims make that a
    // non-issue, so these versions follow the vanilla rule.
    assert_eq!(find_java(&runtimes, 8, JavaKind::Legacy).unwrap().1, 28);
    assert_eq!(find_java(&runtimes, 8, JavaKind::Vanilla).unwrap().1, 28);
}

#[test]
fn a_legacy_launch_goes_through_the_agent() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let runtimes = managed_runtime(tmp.path(), 28);
    let version_dir = tmp.path().join("versions/1.5.2");
    std::fs::create_dir_all(&version_dir).unwrap();
    std::fs::write(
        version_dir.join("1.5.2.json"),
        r#"{"id":"1.5.2","mainClass":"net.minecraft.launchwrapper.Launch","minecraftArguments":"${auth_player_name} --gameDir ${game_directory}","libraries":[]}"#,
    )
    .unwrap();
    std::fs::write(version_dir.join("1.5.2.jar"), b"jar").unwrap();

    let launch = prepare(
        tmp.path(),
        "1.5.2",
        &Credentials::offline("Tester"),
        &Options {
            runtimes: Some(runtimes),
            ..Default::default()
        },
    )
    .unwrap();
    let args = &launch.command.args;
    let agent = version_dir.join("1.5.2.legacy-agent.jar");
    assert!(agent.is_file(), "the agent is written next to the version");
    // The wrapper's own main class cannot start on Java 9+, so the agent
    // supplies the entry point and the class files' applet references.
    assert_eq!(launch.main_class, LEGACY_MAIN);
    assert!(args
        .iter()
        .any(|arg| arg == &format!("-javaagent:{}", agent.display())));
    assert!(
        args.last().is_some_and(|arg| arg == LEGACY_MAIN)
            || args.iter().any(|arg| arg == LEGACY_MAIN)
    );
    // The agent is on the class path too, so training and launch share the
    // same class path (the AOT cache is keyed by it).
    assert!(
        args.iter().any(|arg| arg
            .split(':')
            .any(|entry| entry == agent.display().to_string())),
        "{args:?}"
    );
    // No module patching and no system class loader: both would keep the
    // game's classes out of an AOT cache.
    assert!(!args.iter().any(|arg| arg == "--patch-module"));
    assert!(!args
        .iter()
        .any(|arg| arg.starts_with("-Djava.system.class.loader")));
}

#[test]
fn only_1_8_counts_as_the_netty_4_line() {
    assert!(is_1_8("1.8"));
    assert!(is_1_8("1.8.9"));
    assert!(is_1_8("1.8.9-pre1"));
    assert!(!is_1_8("1.80"));
    assert!(!is_1_8("1.7.10"));
    assert!(!is_1_8("1.9"));
    assert!(!is_1_8("1.12.2"));
}

#[test]
fn a_vanilla_1_8_launch_disables_the_broken_netty_epoll_transport() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    // 8 for the loader profile, 28 for the vanilla one.
    managed_runtime(tmp.path(), 8);
    let runtimes = managed_runtime(tmp.path(), 28);
    let options = Options {
        runtimes: Some(runtimes),
        ..Default::default()
    };

    let launch = prepare(
        tmp.path(),
        "1.8.9",
        &Credentials::offline("Tester"),
        &options,
    )
    .unwrap();
    let arg = launch
        .command
        .args
        .iter()
        .find(|arg| arg.starts_with("-Dio.netty.native.workdir="))
        .expect("1.8.x has to keep netty 4.0's epoll native library out of the JVM");
    // The value has to name something that exists and is *not* a directory:
    // netty's extraction then fails, `Epoll.isAvailable()` answers false and
    // the game falls back to NIO.
    let anchor = Path::new(arg.trim_start_matches("-Dio.netty.native.workdir="));
    assert!(anchor.is_file(), "{anchor:?}");

    // A loader profile is pinned to the Java 8 its metadata asks for, where
    // this netty is healthy, so it keeps epoll.
    let fabric = prepare(
        tmp.path(),
        "1.8.9+Fabric0.15.11",
        &Credentials::offline("Tester"),
        &options,
    )
    .unwrap();
    assert_eq!(fabric.java_major, 8);
    assert!(
        !fabric
            .command
            .args
            .iter()
            .any(|arg| arg.starts_with("-Dio.netty.native.workdir=")),
        "{:?}",
        fabric.command.args
    );
}

#[test]
fn a_vanilla_launch_does_not_carry_the_legacy_agent() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let runtimes = managed_runtime(tmp.path(), 28);
    let launch = prepare(
        tmp.path(),
        "1.8.9",
        &Credentials::offline("Tester"),
        &Options {
            runtimes: Some(runtimes),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        !launch
            .command
            .args
            .iter()
            .any(|arg| arg.starts_with("-javaagent:") || arg == LEGACY_MAIN),
        "{:?}",
        launch.command.args
    );
}

#[test]
fn the_requirement_comes_from_the_profile_or_the_version_number() {
    // What the profile says wins.
    assert_eq!(required_java("1.8.9", Some(17)), 17);
    // Modern versions all carry `javaVersion`; these rules are the fallback.
    assert_eq!(required_java("1.16.5", None), 8);
    assert_eq!(required_java("1.17.1", None), 16);
    assert_eq!(required_java("1.18.2", None), 17);
    assert_eq!(required_java("1.20.4", None), 17);
    assert_eq!(required_java("1.20.5", None), 21);
    assert_eq!(required_java("1.21.9-pre9", None), 21);
    assert_eq!(required_java("26.4-snapshot-7", None), 21);
    // Anything that does not parse is ancient: alphas, betas, web builds.
    assert_eq!(required_java("a1.2.6", None), 8);
    assert_eq!(required_java("b1.7.3", None), 8);
    assert_eq!(required_java("25w37a", None), 8);
}

#[test]
fn java_majors_are_accepted_the_way_the_game_needs_them() {
    assert_eq!(
        parse_java_major("openjdk version \"25.0.1\" 2026-01-20"),
        Some(25)
    );
    assert_eq!(
        parse_java_major("java version \"1.8.0_412\"\nJava(TM) SE"),
        Some(8)
    );
    assert_eq!(
        parse_java_major("openjdk version \"17\" 2021-09-14"),
        Some(17)
    );
    assert_eq!(parse_java_major("no version here"), None);
}

#[test]
fn maven_paths_are_derived_for_libraries_without_an_artifact() {
    assert_eq!(
        maven_path("org.ow2.asm:asm:4.1").as_deref(),
        Some("org/ow2/asm/asm/4.1/asm-4.1.jar")
    );
    assert_eq!(
        maven_path("org.lwjgl:lwjgl:2.9.4:natives-linux").as_deref(),
        Some("org/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4-natives-linux.jar")
    );
    assert_eq!(maven_path("broken"), None);
}

#[test]
fn placeholders_are_replaced_and_unknown_ones_survive() {
    let vars = HashMap::from([("a", "1".to_string()), ("b", "two".to_string())]);
    assert_eq!(substitute("${a}-${b}", &vars), "1-two");
    assert_eq!(substitute("${a}", &vars), "1");
    assert_eq!(substitute("${missing}", &vars), "${missing}");
    assert_eq!(substitute("plain", &vars), "plain");
    assert_eq!(substitute("${unterminated", &vars), "${unterminated");
}

#[test]
fn rules_follow_os_and_last_match_wins() {
    // A rule for another OS never matches.
    let other = if cfg!(windows) { "linux" } else { "windows" };
    let rules = Some(vec![Rule {
        action: "allow".into(),
        os: Some(OsRule {
            name: Some(other.into()),
            arch: None,
        }),
        features: None,
    }]);
    assert!(!rules_allow(&rules, &features(false)));

    // No rules: allowed.
    assert!(rules_allow(&None, &features(false)));

    // allow then disallow -> disallow wins (last match).
    let rules = Some(vec![
        Rule {
            action: "allow".into(),
            os: None,
            features: None,
        },
        Rule {
            action: "disallow".into(),
            os: None,
            features: None,
        },
    ]);
    assert!(!rules_allow(&rules, &features(false)));
}

fn write_version(root: &Path, id: &str, json: serde_json::Value) {
    let dir = root.join("versions").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
}

fn fixture(root: &Path) {
    write_version(
        root,
        "1.8.9",
        serde_json::json!({
            "id": "1.8.9",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "minecraftArguments": "--username ${auth_player_name} --uuid ${auth_uuid} --accessToken ${auth_access_token} --assetsDir ${assets_root} --assetIndex ${assets_index_name} --userType ${user_type}",
            "assetIndex": { "id": "1.8" },
            "javaVersion": { "majorVersion": 8 },
            "libraries": [
                { "name": "com.mojang:patchy:1.1", "downloads": { "artifact": { "path": "com/mojang/patchy/1.1/patchy-1.1.jar" } } },
                { "name": "org.lwjgl:lwjgl:2.9.4", "downloads": { "artifact": { "path": "org/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4.jar" } } }
            ]
        }),
    );
    write_version(
        root,
        "1.8.9+Fabric0.15.11",
        serde_json::json!({
            "id": "1.8.9+Fabric0.15.11",
            "inheritsFrom": "1.8.9",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": { "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main"], "game": [] },
            "libraries": [
                { "name": "net.fabricmc:fabric-loader:0.15.11", "downloads": { "artifact": { "path": "net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar" } } }
            ]
        }),
    );

    // Pretend everything is installed.
    for relative in [
        "com/mojang/patchy/1.1/patchy-1.1.jar",
        "org/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4.jar",
        "net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar",
    ] {
        let path = root.join("libraries").join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"jar").unwrap();
    }
    let jar = root.join("versions/1.8.9/1.8.9.jar");
    std::fs::write(&jar, b"jar").unwrap();
    std::fs::create_dir_all(root.join("versions/1.8.9/1.8.9-natives")).unwrap();

    // A loader install is a copy of the vanilla directory, so the jar inside it
    // still carries the parent's name — which is what the launcher must find.
    let loader_dir = root.join("versions/1.8.9+Fabric0.15.11");
    std::fs::create_dir_all(&loader_dir).unwrap();
    std::fs::copy(&jar, loader_dir.join("1.8.9.jar")).unwrap();
    std::fs::create_dir_all(loader_dir.join("1.8.9-natives")).unwrap();
}

/// A fake Java that answers `-version` with the given major.
fn fake_java(dir: &Path, major: u32) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("java");
    std::fs::write(
        &path,
        format!("#!/bin/sh\necho 'openjdk version \"{major}.0.1\" 2026-01-01' >&2\nexit 0\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    path
}

#[test]
fn natives_placeholders_are_substituted_with_an_absolute_path() {
    // Modern profiles pass four `-D…=${natives_directory}` flags. Leaving them
    // literal makes JNA/LWJGL/Netty create a directory *named*
    // `${natives_directory}` relative to the game directory.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(
        root,
        "26.3",
        serde_json::json!({
            "id": "26.3",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": { "id": "34" },
            "javaVersion": { "majorVersion": 21 },
            "arguments": {
                "jvm": [
                    "-Djava.library.path=${natives_directory}/java",
                    "-Djna.tmpdir=${natives_directory}/jna",
                    "-Dorg.lwjgl.system.SharedLibraryExtractPath=${natives_directory}/lwjgl",
                    "-Dio.netty.native.workdir=${natives_directory}/netty"
                ],
                "game": []
            },
            "libraries": []
        }),
    );
    std::fs::write(root.join("versions/26.3/26.3.jar"), b"jar").unwrap();
    std::fs::create_dir_all(root.join("versions/26.3/26.3-natives")).unwrap();

    let options = Options {
        runtimes: Some(managed_runtime(root, 28)),
        ..Options::default()
    };
    let launch = prepare(root, "26.3", &Credentials::offline("Steve"), &options).unwrap();
    let args = &launch.command.args;
    let natives = root.join("versions/26.3/26.3-natives");

    assert!(
        args.iter()
            .any(|arg| arg == &format!("-Djava.library.path={}/java", natives.display())),
        "the profile's own flag must point at the real directory: {args:?}"
    );
    assert!(
        !args.iter().any(|arg| arg.contains("${")),
        "unsubstituted placeholder: {args:?}"
    );
    assert_eq!(
        args.iter()
            .filter(|arg| arg.starts_with("-Djava.library.path="))
            .count(),
        1,
        "the profile already set it, so the launcher must not add a second: {args:?}"
    );
}

#[test]
fn a_loader_profile_merges_its_parent_and_wins_the_classpath_order() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());

    let creds = Credentials::offline("Steve");
    let options = Options {
        runtimes: Some(managed_runtime(tmp.path(), 8)),
        ..Options::default()
    };
    let launch = prepare(tmp.path(), "1.8.9+Fabric0.15.11", &creds, &options).unwrap();

    assert_eq!(
        launch.main_class,
        "net.fabricmc.loader.impl.launch.knot.KnotClient"
    );
    assert_eq!(launch.required_major, 8);
    assert_eq!(launch.classpath_entries, 4, "3 libraries + client jar");

    let classpath = launch
        .command
        .args
        .iter()
        .position(|arg| arg == "-cp")
        .map(|index| launch.command.args[index + 1].clone())
        .expect("classpath");
    let entries: Vec<&str> = classpath.split(&[':', ';'][..]).collect();
    assert!(
        entries[0].contains("fabric-loader"),
        "child profile first: {classpath}"
    );
    assert!(entries.last().unwrap().ends_with("1.8.9.jar"));

    // The parent's legacy arguments are used because the child has no game args.
    assert!(launch.command.args.iter().any(|arg| arg == "--username"));
    assert!(launch.command.args.iter().any(|arg| arg == "Steve"));
    // The parent's natives folder is what a thin loader profile points at.
    assert_eq!(
        launch.natives.as_deref(),
        Some(
            tmp.path()
                .join("versions/1.8.9+Fabric0.15.11/1.8.9-natives")
                .as_path()
        )
    );
    assert!(launch
        .command
        .args
        .iter()
        .any(|arg| arg.starts_with("-Djava.library.path=")));
    assert!(launch.command.args.iter().any(|arg| arg == "-Xmx2048M"));
}

#[test]
fn a_self_contained_loader_profile_needs_no_vanilla_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let id = "1.20.1-fabric-0.15.11";
    // One directory, exactly what the self-contained install writes: the merged
    // profile, the client jar and the natives. No `versions/1.20.1` at all.
    write_version(
        root,
        id,
        serde_json::json!({
            "id": id,
            "type": "release",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "javaVersion": { "majorVersion": 21 },
            "assetIndex": { "id": "17" },
            "mdtLoader": { "id": "fabric", "version": "0.15.11" },
            "arguments": {
                "jvm": [
                    "-Djava.library.path=${natives_directory}",
                    "-cp",
                    "${classpath}"
                ],
                "game": ["--username", "${auth_player_name}"]
            },
            "libraries": [
                { "name": "net.fabricmc:fabric-loader:0.15.11",
                  "downloads": { "artifact": { "path": "net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar" } } },
                { "name": "com.mojang:patchy:1.1",
                  "downloads": { "artifact": { "path": "com/mojang/patchy/1.1/patchy-1.1.jar" } } }
            ]
        }),
    );
    let version_dir = root.join("versions").join(id);
    for library in [
        "net/fabricmc/fabric-loader/0.15.11/fabric-loader-0.15.11.jar",
        "com/mojang/patchy/1.1/patchy-1.1.jar",
    ] {
        let path = root.join("libraries").join(library);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"jar").unwrap();
    }
    std::fs::write(version_dir.join(format!("{id}.jar")), b"jar").unwrap();
    std::fs::create_dir_all(version_dir.join(format!("{id}-natives"))).unwrap();

    // Both runtimes: the loader rule has to pick the exact pin, not the newest.
    managed_runtime(root, 21);
    let options = Options {
        runtimes: Some(managed_runtime(root, 28)),
        ..Options::default()
    };
    let launch = prepare(root, id, &Credentials::offline("Steve"), &options).unwrap();

    assert!(
        !root.join("versions/1.20.1").exists(),
        "nothing must have to exist beside the loader directory"
    );
    assert_eq!(launch.required_major, 21);
    assert_eq!(launch.java_major, 21, "the loader pin wins over the newest");
    assert_eq!(
        launch.main_class,
        "net.fabricmc.loader.impl.launch.knot.KnotClient"
    );
    assert_eq!(launch.classpath_entries, 3, "2 libraries + client jar");

    let args = &launch.command.args;
    let classpath = args
        .iter()
        .position(|arg| arg == "-cp")
        .map(|index| args[index + 1].clone())
        .expect("classpath");
    let entries: Vec<&str> = classpath.split(&[':', ';'][..]).collect();
    assert!(entries[0].contains("fabric-loader"), "{classpath}");
    assert!(entries.last().unwrap().ends_with(&format!("{id}.jar")));
    // The profile's own natives folder, not a parent's.
    assert_eq!(
        launch.natives.as_deref(),
        Some(version_dir.join(format!("{id}-natives")).as_path())
    );
    // A loader profile never gets the LaunchWrapper shim.
    assert!(!args.iter().any(|arg| arg.starts_with("-javaagent:")));
    assert!(args.iter().any(|arg| arg == "--username"));
}

#[test]
fn a_modern_profile_gets_the_classpath_from_its_own_jvm_arguments() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Every 1.13+ profile ends its jvm arguments with `-cp ${classpath}`. The
    // launcher must not append a second one.
    write_version(
        root,
        "1.20.1",
        serde_json::json!({
            "id": "1.20.1",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "javaVersion": { "majorVersion": 17 },
            "arguments": {
                "jvm": [
                    "-Djava.library.path=${natives_directory}",
                    "-cp",
                    "${classpath}"
                ],
                "game": ["--username", "${auth_player_name}"]
            },
            "libraries": [
                { "name": "com.example:x:1.0", "downloads": { "artifact": { "path": "com/example/x/1.0/x-1.0.jar" } } }
            ]
        }),
    );
    let library = root.join("libraries/com/example/x/1.0/x-1.0.jar");
    std::fs::create_dir_all(library.parent().unwrap()).unwrap();
    std::fs::write(&library, b"jar").unwrap();
    std::fs::write(root.join("versions/1.20.1/1.20.1.jar"), b"jar").unwrap();

    let options = Options {
        runtimes: Some(managed_runtime(root, 28)),
        ..Options::default()
    };
    let launch = prepare(root, "1.20.1", &Credentials::offline("Steve"), &options).unwrap();
    assert_eq!(
        launch
            .command
            .args
            .iter()
            .filter(|arg| *arg == "-cp")
            .count(),
        1,
        "one classpath flag, not two: {:?}",
        launch.command.args
    );
}

#[test]
fn a_profile_whose_id_disagrees_with_its_directory_still_uses_the_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A hand-written instance: the folder is `my-instance`, the profile calls
    // itself `1.20.1` and still inherits from nothing.
    write_version(
        root,
        "my-instance",
        serde_json::json!({
            "id": "1.20.1",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "arguments": { "game": ["--username", "${auth_player_name}"] },
            "libraries": []
        }),
    );
    std::fs::write(root.join("versions/my-instance/my-instance.jar"), b"jar").unwrap();

    let options = Options {
        runtimes: Some(managed_runtime(root, 28)),
        ..Options::default()
    };
    let launch = prepare(
        root,
        "my-instance",
        &Credentials::offline("Steve"),
        &options,
    )
    .unwrap();
    assert_eq!(launch.command.cwd, root.join("versions/my-instance"));
    assert!(
        launch
            .command
            .args
            .windows(2)
            .any(|pair| pair == ["--username", "Steve"]),
        "game arguments are substituted from the profile: {:?}",
        launch.command.args
    );
    // No `javaVersion` in the JSON, so the id it declares (1.20.1 → 17) has to
    // drive the runtime choice, not the folder name.
    assert_eq!(launch.required_major, 17);
}

#[test]
fn placeholders_receive_the_session_values() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let mut creds = Credentials::offline("Steve");
    creds.access_token = "tok".into();
    creds.uuid = "uuid-1".into();
    let options = Options {
        runtimes: Some(managed_runtime(tmp.path(), 28)),
        ..Options::default()
    };
    let launch = prepare(tmp.path(), "1.8.9", &creds, &options).unwrap();
    let joined = launch.command.args.join(" ");
    assert!(joined.contains("--accessToken tok"), "{joined}");
    assert!(joined.contains("--uuid uuid-1"), "{joined}");
    assert!(joined.contains("--assetIndex 1.8"), "{joined}");
    assert!(
        !joined.contains("${"),
        "unsubstituted placeholder: {joined}"
    );
}

#[test]
fn a_missing_library_is_reported_instead_of_launching_half_a_classpath() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    std::fs::remove_file(
        tmp.path()
            .join("libraries/com/mojang/patchy/1.1/patchy-1.1.jar"),
    )
    .unwrap();

    let options = Options {
        runtimes: Some(managed_runtime(tmp.path(), 28)),
        ..Options::default()
    };
    let error = prepare(
        tmp.path(),
        "1.8.9",
        &Credentials::offline("Steve"),
        &options,
    )
    .expect_err("must refuse");
    assert!(error.to_string().contains("依赖文件缺失"), "{error}");
}

#[test]
fn a_missing_client_jar_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    std::fs::remove_file(tmp.path().join("versions/1.8.9/1.8.9.jar")).unwrap();
    let options = Options {
        runtimes: Some(managed_runtime(tmp.path(), 28)),
        ..Options::default()
    };
    let error = prepare(
        tmp.path(),
        "1.8.9",
        &Credentials::offline("Steve"),
        &options,
    )
    .expect_err("must refuse");
    assert!(error.to_string().contains("客户端 jar"), "{error}");
}

#[test]
fn a_profile_without_java_version_gets_it_from_the_version_number() {
    let tmp = tempfile::tempdir().unwrap();
    // 1.20.5 is the first release that needs 21; the metadata here has no
    // `javaVersion`, so only the version id can decide.
    write_version(
        tmp.path(),
        "1.20.5",
        serde_json::json!({
            "id": "1.20.5",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": { "id": "19" },
            "libraries": []
        }),
    );
    std::fs::write(tmp.path().join("versions/1.20.5/1.20.5.jar"), b"jar").unwrap();
    let options = Options {
        runtimes: Some(managed_runtime(tmp.path(), 28)),
        ..Options::default()
    };
    let launch = prepare(
        tmp.path(),
        "1.20.5",
        &Credentials::offline("Steve"),
        &options,
    )
    .unwrap();
    assert_eq!(launch.required_major, 21, "1.20.5 needs 21");
    // The minimum says 21, but vanilla is locked to 28 either way.
    assert_eq!(launch.java_major, 28);
}

#[test]
fn a_profile_newer_than_java_28_is_refused_with_what_was_found() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let options = Options {
        runtimes: Some(managed_runtime(tmp.path(), 28)),
        ..Options::default()
    };
    // A profile that wants a runtime newer than the one the shims know about
    // cannot be guessed at: 28 would reject the class files anyway.
    write_version(
        tmp.path(),
        "1.30",
        serde_json::json!({
            "id": "1.30",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "javaVersion": { "majorVersion": 30 },
            "libraries": []
        }),
    );
    // Give it a jar so only the Java decision is under test.
    std::fs::write(tmp.path().join("versions/1.30/1.30.jar"), b"jar").unwrap();
    let error = prepare(tmp.path(), "1.30", &Credentials::offline("Steve"), &options)
        .expect_err("Java 28 cannot run a 30 profile");
    assert!(error.to_string().contains("Java 30"), "{error}");
    assert!(error.to_string().contains("Java 28"), "{error}");
}

#[test]
fn a_vanilla_profile_below_28_still_asks_for_28() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let options = Options {
        runtimes: Some(managed_runtime(tmp.path(), 21)),
        ..Options::default()
    };
    // A 21 runtime meets every minimum here and is still not used: vanilla is
    // locked to the one runtime the shims were tested on.
    let error = prepare(
        tmp.path(),
        "1.8.9",
        &Credentials::offline("Steve"),
        &options,
    )
    .expect_err("a 21 runtime is not Java 28");
    assert!(error.to_string().contains("Java 28"), "{error}");
    assert!(error.to_string().contains("Java 21"), "{error}");
    assert!(error.to_string().contains("Java 运行时界面"), "{error}");
}

#[test]
fn account_files_give_up_a_session_or_an_explanation() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("Steve.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "format": 1,
            "application": "Minecraft Launcher",
            "app_id": "id",
            "player": { "name": "Steve", "uuid": "11111111-1111-1111-1111-111111111111" },
            "session": { "minecraft_token": { "access_token": "tok", "xuid": "42" } }
        })
        .to_string(),
    )
    .unwrap();
    let creds = Credentials::from_account_file(&path).unwrap();
    assert_eq!(creds.user_type, "msa");
    assert_eq!(creds.access_token, "tok");
    assert_eq!(creds.xuid.as_deref(), Some("42"));
    assert_eq!(creds.username, "Steve");

    // A file without a Minecraft token must say so, not fall back silently.
    std::fs::write(&path, r#"{"player":{"name":"Steve"}}"#).unwrap();
    let error = Credentials::from_account_file(&path).unwrap_err();
    assert!(error.to_string().contains("重新登录"), "{error}");
}

#[test]
fn a_chain_that_loops_is_an_error_not_a_stack_of_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    write_version(
        tmp.path(),
        "a",
        serde_json::json!({"id": "a", "inheritsFrom": "b", "mainClass": "A"}),
    );
    write_version(
        tmp.path(),
        "b",
        serde_json::json!({"id": "b", "inheritsFrom": "a", "mainClass": "B"}),
    );
    let error = load_chain(tmp.path(), "a").unwrap_err();
    assert!(error.to_string().contains("有环"), "{error}");
}

#[test]
fn a_chain_deeper_than_the_limit_says_so_instead_of_launching_half_of_it() {
    let tmp = tempfile::tempdir().unwrap();
    for index in 0..=MAX_CHAIN_DEPTH {
        let id = format!("v{index}");
        let parent = (index < MAX_CHAIN_DEPTH).then(|| format!("v{}", index + 1));
        let mut body = serde_json::json!({"id": id, "mainClass": "Main"});
        if let Some(parent) = parent {
            body["inheritsFrom"] = serde_json::Value::String(parent);
        }
        write_version(tmp.path(), &id, body);
    }
    let error = load_chain(tmp.path(), "v0").unwrap_err();
    assert!(error.to_string().contains("超过"), "{error}");
}

#[test]
fn custom_resolution_is_announced_to_the_game() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let options = Options {
        runtimes: Some(managed_runtime(tmp.path(), 28)),
        width: 1280,
        height: 720,
        ..Options::default()
    };
    let launch = prepare(
        tmp.path(),
        "1.8.9",
        &Credentials::offline("Steve"),
        &options,
    )
    .unwrap();
    let joined = launch.command.args.join(" ");
    assert!(joined.contains("1280"), "{joined}");
    assert!(joined.contains("720"), "{joined}");
}

#[test]
fn a_loader_profile_can_borrow_the_parents_jar_and_natives() {
    let tmp = tempfile::tempdir().unwrap();
    let versions = tmp.path().join("versions");
    let parent = versions.join("1.12.2");
    std::fs::create_dir_all(parent.join("1.12.2-natives")).unwrap();
    std::fs::write(parent.join("1.12.2.jar"), b"jar").unwrap();
    // A legacy Forge install keeps its own directory and inherits 1.12.2.
    let child = versions.join("1.12.2+Forge14.23.0.2486");
    std::fs::create_dir_all(&child).unwrap();
    let parents = ["1.12.2".to_string()];

    assert_eq!(
        client_jar(&child, "1.12.2+Forge14.23.0.2486", &parents),
        Some(parent.join("1.12.2.jar"))
    );
    assert_eq!(
        find_natives(&child, "1.12.2+Forge14.23.0.2486", &parents),
        Some(parent.join("1.12.2-natives"))
    );
}

#[cfg(unix)]
#[test]
fn a_lwjgl_32_natives_path_is_mirrored_when_the_data_directory_is_not_ascii() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("游戏");
    write_version(
        &root,
        "1.16.5",
        serde_json::json!({
            "id": "1.16.5",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": { "id": "1.16" },
            "javaVersion": { "majorVersion": 8 },
            "arguments": { "jvm": ["-Djava.library.path=${natives_directory}"], "game": [] },
            "libraries": [
                { "name": "org.lwjgl:lwjgl:3.2.2", "downloads": { "artifact": { "path": "org/lwjgl/lwjgl/3.2.2/lwjgl-3.2.2.jar" } } }
            ]
        }),
    );
    std::fs::write(root.join("versions/1.16.5/1.16.5.jar"), b"jar").unwrap();
    let natives = root.join("versions/1.16.5/1.16.5-natives");
    std::fs::create_dir_all(&natives).unwrap();
    let library = root.join("libraries/org/lwjgl/lwjgl/3.2.2/lwjgl-3.2.2.jar");
    std::fs::create_dir_all(library.parent().unwrap()).unwrap();
    std::fs::write(&library, b"jar").unwrap();

    let options = Options {
        runtimes: Some(managed_runtime(&root, 28)),
        ..Options::default()
    };
    let launch = prepare(&root, "1.16.5", &Credentials::offline("Steve"), &options).unwrap();
    let flag = launch
        .command
        .args
        .iter()
        .find(|arg| arg.starts_with("-Djava.library.path="))
        .expect("a library path");
    let path = Path::new(flag.trim_start_matches("-Djava.library.path="));
    assert!(path.as_os_str().is_ascii(), "mirror is not ASCII: {path:?}");
    assert_eq!(
        std::fs::canonicalize(path).unwrap(),
        std::fs::canonicalize(&natives).unwrap()
    );
}

#[test]
fn only_lwjgl_31_and_32_need_the_ascii_natives_mirror() {
    let path = |version: &str| {
        vec![PathBuf::from(format!(
            "libraries/org/lwjgl/lwjgl/{version}/lwjgl-{version}.jar"
        ))]
    };
    assert!(lwjgl_needs_ascii_natives(&path("3.1.6")));
    assert!(lwjgl_needs_ascii_natives(&path("3.2.2")));
    assert!(!lwjgl_needs_ascii_natives(&path("2.9.4")));
    assert!(!lwjgl_needs_ascii_natives(&path("3.3.3")));
    assert!(!lwjgl_needs_ascii_natives(&path("3.4.3")));
}

#[cfg(unix)]
#[test]
fn a_non_ascii_natives_path_gets_an_ascii_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let natives = tmp.path().join("游戏").join("1.16.5-natives");
    std::fs::create_dir_all(&natives).unwrap();
    std::fs::write(natives.join("libglfw.so"), b"x").unwrap();

    let mirror = ascii_natives_mirror(&natives).expect("a mirror");
    assert!(mirror.as_os_str().is_ascii(), "{mirror:?}");
    assert_eq!(std::fs::read_link(&mirror).unwrap(), natives);
    assert_eq!(
        std::fs::read(mirror.join("libglfw.so")).unwrap(),
        b"x".to_vec()
    );
    // An ASCII natives path is left alone.
    assert!(ascii_natives_mirror(tmp.path()).is_none());
}

#[test]
fn the_lwjgl3_mode_swaps_the_libraries_and_adds_the_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A 1.8.9-shaped profile: it lists an LWJGL 2 library the mode has to drop.
    // The file itself is never created, which is the point: the mode must not
    // need the LWJGL 2 jars to be present at all.
    write_version(
        root,
        "1.8.9",
        serde_json::json!({
            "id": "1.8.9",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "minecraftArguments": "--username ${auth_player_name} --gameDir ${game_directory}",
            "libraries": [
                { "name": "org.lwjgl.lwjgl:lwjgl:2.9.4",
                  "downloads": { "artifact": { "path": "org/lwjgl/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4.jar" } } },
                { "name": "com.mojang:patchy:1.1",
                  "downloads": { "artifact": { "path": "com/mojang/patchy/1.1/patchy-1.1.jar" } } }
            ]
        }),
    );
    // A real (empty) zip: the mode strips the jar's signature files, so the
    // client jar has to be parseable.
    let client = root.join("versions/1.8.9/1.8.9.jar");
    {
        use std::io::Write;
        let file = std::fs::File::create(&client).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            "META-INF/MANIFEST.MF",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"Manifest-Version: 1.0\n").unwrap();
        zip.finish().unwrap();
    }
    let patchy = root.join("libraries/com/mojang/patchy/1.1/patchy-1.1.jar");
    std::fs::create_dir_all(patchy.parent().unwrap()).unwrap();
    std::fs::write(&patchy, b"jar").unwrap();

    // The LWJGL 3 jars the mode expects, as empty files: only their existence
    // matters for building the command.
    let runtimes = managed_runtime(root, 28);
    let lwjgl3_dir = crate::lwjgl3::dir(&runtimes);
    std::fs::create_dir_all(&lwjgl3_dir).unwrap();
    let jars = crate::lwjgl3::jars(&lwjgl3_dir).unwrap();
    for jar in &jars {
        std::fs::write(jar, b"jar").unwrap();
    }

    let options = Options {
        runtimes: Some(runtimes),
        lwjgl3: true,
        ..Options::default()
    };
    let launch = prepare(root, "1.8.9", &Credentials::offline("Steve"), &options).unwrap();
    let args = &launch.command.args;
    let classpath = args
        .iter()
        .position(|arg| arg == "-cp")
        .map(|index| args[index + 1].clone())
        .expect("classpath");
    assert!(
        !classpath.contains("lwjgl-2.9.4"),
        "the LWJGL 2 jar must be gone: {classpath}"
    );
    assert!(classpath.contains("patchy-1.1.jar"), "{classpath}");
    for jar in &jars {
        assert!(
            classpath.contains(&jar.display().to_string()),
            "{} missing from {classpath}",
            jar.display()
        );
    }
    assert_eq!(launch.classpath_entries, 1 + jars.len() + 1);
    // The agent, and the flag LWJGL 3 asks for on JDK 24+.
    let agent = crate::lwjgl3::agent(&lwjgl3_dir).unwrap();
    assert!(agent.is_file(), "the embedded agent is written out");
    assert!(
        args.iter()
            .any(|arg| arg == &format!("-javaagent:{}", agent.display())),
        "{args:?}"
    );
    assert!(args
        .iter()
        .any(|arg| arg == "--enable-native-access=ALL-UNNAMED"));
    // LWJGL 3 extracts its own natives, so no library path is set.
    assert!(
        !args
            .iter()
            .any(|arg| arg.starts_with("-Djava.library.path=")),
        "{args:?}"
    );
}

#[test]
fn the_lwjgl3_mode_leaves_a_loader_profie_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_version(
        root,
        "1.8.9-forge-11.15.0.1656",
        serde_json::json!({
            "id": "1.8.9-forge-11.15.0.1656",
            "type": "release",
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "mdtLoader": { "id": "forge", "version": "11.15.0.1656" },
            "libraries": []
        }),
    );
    std::fs::write(
        root.join("versions/1.8.9-forge-11.15.0.1656/1.8.9-forge-11.15.0.1656.jar"),
        b"jar",
    )
    .unwrap();
    managed_runtime(root, 8);

    let options = Options {
        runtimes: Some(root.join("运行时")),
        lwjgl3: true,
        ..Options::default()
    };
    // A loader profile keeps its own Java pin, so this needs the Java 8 runtime
    // created above; the point of the test is that no agent is added.
    let launch = prepare(
        root,
        "1.8.9-forge-11.15.0.1656",
        &Credentials::offline("Steve"),
        &options,
    )
    .unwrap();
    assert!(
        !launch
            .command
            .args
            .iter()
            .any(|arg| arg.starts_with("-javaagent:tessera") || arg.contains("lwjgl3-agent")),
        "loader profiles are not rewritten: {:?}",
        launch.command.args
    );
}
