//! Manual verification tool for the identities this crate can sign in as.
//!
//! Not part of the library API and not covered by tests: it talks to the real
//! Microsoft/Xbox/Mojang services and needs a human to complete a sign-in.
//! It exists because two things can only be checked against the live services:
//! whether an identity's sign-in chain works, and whether its session refreshes.
//!
//! ```sh
//! # Official launcher identity (title client id + device token + SISU).
//! cargo run --example verify_login
//!
//! # A third-party Azure application, e.g. Prism Launcher's client id.
//! cargo run --example verify_login -- --client-id c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb
//!
//! # Load the saved session (it carries its own identity) and force a refresh.
//! cargo run --example verify_login -- --resume
//! ```
//!
//! What to look for:
//!
//! - every stage prints its own line; the failing stage is the error's variant
//!   (`Error::Msa` → polling/refresh, `Error::Xbl` → device token or SISU,
//!   `Error::MinecraftServices` → `login_with_xbox`);
//! - `refresh:` right after sign-in, and again on `--resume`;
//! - `xuid` present, `player_name`/`player_uuid` belonging to the account.
//!
//! Passing this does **not** prove the session works: an invalid token still
//! starts the game in single-player. Launch a real client with the printed
//! arguments and join an online-mode server or a Realm.
//!
//! The saved session holds live tokens (`--accessToken`, the refresh token) —
//! delete it when done.

use std::path::Path;

use minecraft_auth::java::JavaAuthManager;
use minecraft_auth::msa::{constants, MsaApplicationConfig, MsaToken};
use minecraft_auth::AuthApplication;

const SESSION_PATH: &str = "session.json";

struct Options {
    client_id: Option<String>,
    scope: Option<String>,
    resume: bool,
}

fn parse_options() -> Options {
    let mut options = Options {
        client_id: None,
        scope: None,
        resume: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--resume" => options.resume = true,
            "--client-id" => options.client_id = args.next(),
            "--scope" => options.scope = args.next(),
            other => {
                eprintln!("unknown argument {other:?}; see the doc comment at the top");
                std::process::exit(2);
            }
        }
    }
    options
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options();
    let http = reqwest::Client::new();

    if options.resume {
        let manager = resume(&http).await?;
        report(&manager).await?;
        save(&manager).await?;
        return Ok(());
    }

    // A client id means a third-party Azure application; no client id means the
    // official launcher's title identity.
    let config = match &options.client_id {
        Some(client_id) => MsaApplicationConfig::azure_application(
            client_id,
            options
                .scope
                .clone()
                .unwrap_or_else(|| constants::SCOPE_OFFLINE_ACCESS.to_string()),
        ),
        None => {
            let config = MsaApplicationConfig::java_default();
            match &options.scope {
                Some(scope) => MsaApplicationConfig {
                    scope: scope.clone(),
                    ..config
                },
                None => config,
            }
        }
    };
    config.validate()?;
    let application = AuthApplication::new(
        match &options.client_id {
            Some(client_id) => format!("third-party ({client_id})"),
            None => "Minecraft Launcher".to_string(),
        },
        config,
    );
    describe(&application);

    let manager = sign_in(&http, &application).await?;
    report(&manager).await?;
    save(&manager).await?;
    Ok(())
}

fn describe(application: &AuthApplication) {
    println!(
        "identity: {} ({})\nchain:    {:?} / {:?}",
        application.label,
        application.config.client_id,
        application.config.xbl_chain(),
        application.config.environment
    );
}

/// Device-code sign-in, a refresh canary, then persist the session.
async fn sign_in(
    http: &reqwest::Client,
    application: &AuthApplication,
) -> Result<JavaAuthManager, Box<dyn std::error::Error>> {
    let manager = JavaAuthManager::builder(http.clone())
        .application(application)
        .login_device_code(|code| {
            println!("\nopen:  {}", code.direct_verification_uri());
            println!("code:  {}\n", code.user_code);
            println!("waiting for sign-in…");
        })
        .await?;
    println!("msa:      signed in");
    save(&manager).await?;

    // Refresh canary: a launcher lives or dies by this, and it is not the same
    // code path as signing in.
    let mut probe = manager.to_json().await?;
    probe["msa_token"]["expire_time_ms"] = serde_json::json!(0);
    let probe = JavaAuthManager::from_json_for_application(http.clone(), &probe, application)?;
    match probe.msa_token().await {
        Ok(token) => {
            println!("refresh:  ok, new expiry {}", token.expire_time_ms);
            // A refresh rotates the refresh token, so keep the file current.
            save(&probe).await?;
        }
        Err(error) => println!("refresh:  FAILED — {error:?}"),
    }
    Ok(manager)
}

/// Loads the saved session (which carries its own identity), forces the MSA
/// token to expire and refreshes it — what a launcher does every day.
async fn resume(http: &reqwest::Client) -> Result<JavaAuthManager, Box<dyn std::error::Error>> {
    println!("resuming: {SESSION_PATH}");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(Path::new(SESSION_PATH))?)?;

    // The session carries the identity it was signed in with; no flag needed.
    let config: MsaApplicationConfig = serde_json::from_value(json["application_config"].clone())?;
    config.validate()?;
    let application = AuthApplication::new(format!("saved session ({})", config.client_id), config);
    describe(&application);

    json["msa_token"]["expire_time_ms"] = serde_json::json!(0);
    let manager = JavaAuthManager::from_json_for_application(http.clone(), &json, &application)?;

    let refreshed: MsaToken = manager.msa_token().await?;
    println!(
        "msa:      refreshed, new expiry {} (refresh token: {})",
        refreshed.expire_time_ms,
        if refreshed.refresh_token.is_some() {
            "rotated"
        } else {
            "absent"
        }
    );
    save(&manager).await?;
    Ok(manager)
}

/// Writes the whole cached chain back, so the file holds the newest tokens.
async fn save(manager: &JavaAuthManager) -> Result<(), Box<dyn std::error::Error>> {
    let json = manager.to_json().await?;
    std::fs::write(SESSION_PATH, serde_json::to_vec_pretty(&json)?)?;
    Ok(())
}

/// Walks every stage so a failure names the step it died on, then prints the
/// game's arguments.
async fn report(manager: &JavaAuthManager) -> Result<(), Box<dyn std::error::Error>> {
    let token = manager.minecraft_token().await?;
    println!(
        "minecraft: {} token, expires {}",
        token.token_type, token.expire_time_ms
    );

    let profile = manager.profile().await?;
    println!("profile:  {} ({})", profile.name, profile.id);

    let session = manager.launch_session().await?;
    println!(
        "xuid:     {}",
        session
            .xuid
            .as_deref()
            .unwrap_or("<missing — the access token carried no xuid claim>")
    );

    println!("\nlaunch arguments:");
    println!("  --username    {}", session.player_name);
    println!("  --uuid        {}", session.player_uuid);
    println!("  --accessToken {}", session.access_token);
    println!("  --userType    msa");
    if let Some(xuid) = &session.xuid {
        println!("  --xuid        {xuid}");
    }
    println!(
        "\nnext: start a real client with these and join an online-mode server.\n\
         until that works, this session is unverified."
    );
    Ok(())
}
