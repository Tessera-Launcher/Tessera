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
//!
//! # Experiment: refresh before SISU, run the chain, refresh after SISU.
//! cargo run --example verify_login -- --probe
//!
//! # Control for --probe: refresh twice, but never run the chain.
//! cargo run --example verify_login -- --control
//!
//! # Keep each trial's tokens in their own file.
//! cargo run --example verify_login -- --probe --session probe-1.json
//!
//! # Sign in with the authorization-code flow instead of a device code, then
//! # run the same four probe steps (official title identity only).
//! cargo run --example verify_login -- --auth-code --session authcode-1.json
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
//! `--probe` and `--control` exist for one question: does using the MBI_SSL
//! ticket in the Xbox chain make its refresh token unredeemable? Both force the
//! MSA token to expire, so every refresh is a real network request. Every
//! successful refresh rotates the refresh token, so the session file is written
//! after every step — losing a rotation would make the next step look like a
//! dead account for the wrong reason.
//!
//! Passing `verify_login` does **not** prove the session works: an invalid token
//! still starts the game in single-player. Launch a real client with the printed
//! arguments and join an online-mode server or a Realm.
//!
//! The saved session holds live tokens (`--accessToken`, the refresh token) —
//! delete it when done.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use minecraft_auth::java::JavaAuthManager;
use minecraft_auth::msa::{constants, MsaApplicationConfig, MsaEnvironment};
use minecraft_auth::{AuthApplication, EcKeyPair, Error};

const DEFAULT_SESSION: &str = "session.json";
const AUTH_CODE_REDIRECT: &str = "https://login.live.com/oauth20_desktop.srf";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Default,
    Resume,
    Probe,
    Control,
    AuthCode,
}

struct Options {
    client_id: Option<String>,
    scope: Option<String>,
    session: PathBuf,
    mode: Mode,
}

fn die(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}

fn set_mode(current: &mut Mode, next: Mode) {
    if *current != Mode::Default && *current != next {
        die("choose only one of --resume, --probe, --control");
    }
    *current = next;
}

fn parse_options() -> Options {
    let mut options = Options {
        client_id: None,
        scope: None,
        session: PathBuf::from(DEFAULT_SESSION),
        mode: Mode::Default,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--resume" => set_mode(&mut options.mode, Mode::Resume),
            "--probe" => set_mode(&mut options.mode, Mode::Probe),
            "--control" => set_mode(&mut options.mode, Mode::Control),
            "--auth-code" => set_mode(&mut options.mode, Mode::AuthCode),
            "--client-id" => match args.next() {
                Some(value) => options.client_id = Some(value),
                None => die("--client-id needs a value"),
            },
            "--scope" => match args.next() {
                Some(value) => options.scope = Some(value),
                None => die("--scope needs a value"),
            },
            "--session" => match args.next() {
                Some(value) => options.session = PathBuf::from(value),
                None => die("--session needs a path"),
            },
            other => die(&format!(
                "unknown argument {other:?}; see the doc comment at the top"
            )),
        }
    }
    options
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options();
    let http = reqwest::Client::new();

    match options.mode {
        Mode::Resume => resume(&http, &options.session).await,
        Mode::Probe => probe(&http, &options).await,
        Mode::Control => control(&http, &options).await,
        Mode::AuthCode => auth_code(&http, &options).await,
        Mode::Default => default_flow(&http, &options).await,
    }
}

fn build_application(options: &Options) -> Result<AuthApplication, Box<dyn std::error::Error>> {
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
    Ok(AuthApplication::new(
        match &options.client_id {
            Some(client_id) => format!("third-party ({client_id})"),
            None => "Minecraft Launcher".to_string(),
        },
        config,
    ))
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

/// The original flow: sign in, refresh canary, walk every stage.
async fn default_flow(
    http: &reqwest::Client,
    options: &Options,
) -> Result<(), Box<dyn std::error::Error>> {
    let application = build_application(options)?;
    describe(&application);
    let manager = sign_in(http, &application, &options.session).await?;
    report(&manager).await?;
    save(&manager, &options.session).await?;
    Ok(())
}

/// Device-code sign-in, a refresh canary, then persist the session.
///
/// Returns the manager that owns the newest tokens: when the canary succeeds it
/// is the refreshed one, so the chain below (and the final save) cannot write
/// the already-rotated refresh token back over its replacement.
async fn sign_in(
    http: &reqwest::Client,
    application: &AuthApplication,
    path: &Path,
) -> Result<JavaAuthManager, Box<dyn std::error::Error>> {
    let manager = login(http, application, path).await?;
    let session = manager.to_json().await?;

    // Refresh canary: a launcher lives or dies by this, and it is not the same
    // code path as signing in.
    println!("refresh:  forcing the MSA token to expire");
    match refresh_msa(http, &session, application).await {
        Ok(refreshed) => {
            println!("refresh:  ok — {}", summary(&session, &refreshed.session));
            // A refresh rotates the refresh token, so keep the file current.
            save_json(path, &refreshed.session)?;
            Ok(refreshed.manager)
        }
        Err(error) => {
            println!("refresh:  FAILED — {error}");
            Ok(manager)
        }
    }
}

async fn login(
    http: &reqwest::Client,
    application: &AuthApplication,
    path: &Path,
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
    save(&manager, path).await?;
    println!("saved:    {}", path.display());
    Ok(manager)
}

/// Authorization-code sign-in for the official title identity.
///
/// The title client's only redirect is `oauth20_desktop.srf`; that page shows
/// the code in the URL. Asking the user to paste that URL is the whole
/// "webview", and the exchange needs no client secret.
async fn auth_code_login(
    http: &reqwest::Client,
    application: &AuthApplication,
    path: &Path,
) -> Result<JavaAuthManager, Box<dyn std::error::Error>> {
    let authorize = format!(
        "https://login.live.com/oauth20_authorize.srf?client_id={}&response_type=code&scope={}&redirect_uri={}",
        query_encode(&application.config.client_id),
        query_encode(&application.config.scope),
        query_encode(AUTH_CODE_REDIRECT),
    );
    println!("\nopen:  {authorize}");
    println!("after signing in, paste the full URL that starts with");
    println!("{AUTH_CODE_REDIRECT}?code=…");
    print!("url:   ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let code = extract_code(&line)
        .ok_or_else(|| std::io::Error::other("no code= found in the pasted URL"))?;

    let response = http
        .post(application.config.environment.token_url())
        .form(&[
            ("client_id", application.config.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", AUTH_CODE_REDIRECT),
            ("scope", application.config.scope.as_str()),
        ])
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        println!("token exchange: {status} {}", truncate(&body, 600));
        return Err(std::io::Error::other(format!(
            "authorization-code exchange failed with {status}"
        ))
        .into());
    }

    let value: serde_json::Value = serde_json::from_str(&body)?;
    let access_token = value["access_token"]
        .as_str()
        .ok_or("token exchange: no access_token")?;
    let refresh_token = value["refresh_token"]
        .as_str()
        .ok_or("token exchange: no refresh_token")?;
    let expires_in = expires_in_seconds(&value).ok_or("token exchange: no numeric expires_in")?;

    let session = serde_json::json!({
        "_save_version": 1,
        "application_config": serde_json::to_value(&application.config)?,
        "device_type": "Win32",
        "device_key_pair": EcKeyPair::generate(),
        "device_id": uuid::Uuid::new_v4().to_string(),
        "msa_token": {
            "expire_time_ms": now_ms() + expires_in * 1000,
            "access_token": access_token,
            "refresh_token": refresh_token,
        }
    });
    let manager = JavaAuthManager::from_json_for_application(http.clone(), &session, application)?;
    save_json(path, &session)?;
    println!("msa:      signed in with an authorization code");
    println!("saved:    {}", path.display());
    Ok(manager)
}

/// `--probe`: refresh before any Xbox call, run the chain, refresh again.
///
/// The launcher signs in with `launch_session()`, so its refresh token has
/// already been through device/authenticate, SISU and login_with_xbox before a
/// launcher ever tries to refresh it. This reproduces both orders in one login.
async fn probe(
    http: &reqwest::Client,
    options: &Options,
) -> Result<(), Box<dyn std::error::Error>> {
    let application = build_application(options)?;
    describe(&application);
    println!("session:  {}", options.session.display());

    let manager = login(http, &application, &options.session).await?;
    run_probe(http, &application, &options.session, manager).await
}

/// `--auth-code`: sign in with the authorization-code flow, then run the same
/// four probe steps.
///
/// The title client only accepts `https://login.live.com/oauth20_desktop.srf`
/// as its redirect. That page shows the code in the address bar; paste the
/// whole URL back here. No webview and no secret are involved.
async fn auth_code(
    http: &reqwest::Client,
    options: &Options,
) -> Result<(), Box<dyn std::error::Error>> {
    if options.client_id.is_some() {
        die("--auth-code is wired for the official title identity only; drop --client-id");
    }
    let application = build_application(options)?;
    if application.config.environment != MsaEnvironment::Live {
        die("--auth-code needs the Live environment");
    }
    describe(&application);
    println!("session:  {}", options.session.display());

    let manager = auth_code_login(http, &application, &options.session).await?;
    run_probe(http, &application, &options.session, manager).await
}

/// The four steps shared by `--probe` and `--auth-code`.
async fn run_probe(
    http: &reqwest::Client,
    application: &AuthApplication,
    path: &Path,
    manager: JavaAuthManager,
) -> Result<(), Box<dyn std::error::Error>> {
    let session = manager.to_json().await?;
    save_json(path, &session)?;

    println!(
        "\n[{}] 1/4  refresh before the chain (no SISU yet)",
        stamp()
    );
    let before = match refresh_msa(http, &session, application).await {
        Ok(refreshed) => refreshed,
        Err(error) => {
            println!("result:   FAILED — {error}");
            println!(
                "verdict:  the fresh title token cannot refresh even before SISU; \
                 this is the token class/endpoint, not the chain."
            );
            return Ok(());
        }
    };
    println!("result:   ok — {}", summary(&session, &before.session));
    save_json(path, &before.session)?;

    println!(
        "\n[{}] 2/4  chain: device/authenticate + SISU + login_with_xbox + profile",
        stamp()
    );
    let chain = before.manager.launch_session().await;
    // Persist whatever the chain produced even if it failed: a rotated MSA
    // token must never be lost because a later stage errored.
    let after_chain = before.manager.to_json().await?;
    save_json(path, &after_chain)?;
    match chain {
        Ok(session) => println!(
            "result:   ok — {} ({})",
            session.player_name, session.player_uuid
        ),
        Err(error) => {
            println!("result:   FAILED — {error:?}");
            println!("saved:    the rotated MSA token is in {}", path.display());
            return Ok(());
        }
    }

    println!("\n[{}] 3/4  refresh after the chain", stamp());
    match refresh_msa(http, &after_chain, application).await {
        Ok(refreshed) => {
            println!(
                "result:   ok — {}",
                summary(&after_chain, &refreshed.session)
            );
            save_json(path, &refreshed.session)?;
            println!(
                "\n[{}] 4/4  verdict: both refreshes succeeded, so SISU is not the cause.\n\
                 next:      run --control. If its second refresh also succeeds, the failure is\n\
                 time/another client, not the chain.",
                stamp()
            );
        }
        Err(error) => {
            println!("result:   FAILED — {error}");
            println!(
                "\n[{}] 4/4  verdict: the pre-SISU refresh worked and the post-SISU refresh failed.\n\
                 The Xbox chain (device/SISU/ticket use), not the token's age, is implicated.",
                stamp()
            );
        }
    }
    Ok(())
}

/// `--control`: the same two refreshes as `--probe`, without the chain.
async fn control(
    http: &reqwest::Client,
    options: &Options,
) -> Result<(), Box<dyn std::error::Error>> {
    let application = build_application(options)?;
    describe(&application);
    println!("session:  {}", options.session.display());

    let manager = login(http, &application, &options.session).await?;
    let session = manager.to_json().await?;
    save_json(&options.session, &session)?;

    println!("\n[{}] 1/3  refresh #1", stamp());
    let first = match refresh_msa(http, &session, &application).await {
        Ok(refreshed) => refreshed,
        Err(error) => {
            println!("result:   FAILED — {error}");
            return Ok(());
        }
    };
    println!("result:   ok — {}", summary(&session, &first.session));
    save_json(&options.session, &first.session)?;

    println!("\n[{}] 2/3  refresh #2 (never ran the chain)", stamp());
    match refresh_msa(http, &first.session, &application).await {
        Ok(second) => {
            println!(
                "result:   ok — {}",
                summary(&first.session, &second.session)
            );
            save_json(&options.session, &second.session)?;
            println!(
                "\n[{}] 3/3  verdict: refresh-after-refresh works without the chain.\n\
                 If --probe's post-chain refresh failed, the chain is the differentiator.",
                stamp()
            );
        }
        Err(error) => {
            println!("result:   FAILED — {error}");
            println!(
                "\n[{}] 3/3  verdict: a second refresh in a row fails even without the chain;\n\
                 rotation itself / time is the cause, not SISU.",
                stamp()
            );
        }
    }
    Ok(())
}

/// Loads the saved session (which carries its own identity), forces the MSA
/// token to expire and refreshes it — what a launcher does every day.
async fn resume(http: &reqwest::Client, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("resuming: {}", path.display());
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;

    // The session carries the identity it was signed in with; no flag needed.
    let config: MsaApplicationConfig = serde_json::from_value(json["application_config"].clone())?;
    config.validate()?;
    let application = AuthApplication::new(format!("saved session ({})", config.client_id), config);
    describe(&application);

    println!("\n[{}] refresh (MSA forced to expire)", stamp());
    let refreshed = refresh_msa(http, &json, &application).await?;
    println!("result:   ok — {}", summary(&json, &refreshed.session));
    save_json(path, &refreshed.session)?;
    report(&refreshed.manager).await?;
    save(&refreshed.manager, path).await?;
    Ok(())
}

/// A session after a successful refresh, plus the manager holding the new token.
struct RefreshOk {
    manager: JavaAuthManager,
    session: serde_json::Value,
}

/// Force one MSA refresh through the library, then persist nothing itself.
///
/// On a 5xx the refresh is retried once with the same request, because the
/// library's `Error` does not carry Microsoft's response body and `foci` /
/// `correlation_id` only exist there. If the retry succeeds the failure was
/// transient; its tokens are returned and must be saved like any other
/// rotation. On a 4xx the library error already says what happened, so it is
/// returned unchanged.
async fn refresh_msa(
    http: &reqwest::Client,
    session: &serde_json::Value,
    application: &AuthApplication,
) -> Result<RefreshOk, Box<dyn std::error::Error>> {
    let mut probe = session.clone();
    probe["msa_token"]["expire_time_ms"] = serde_json::json!(0);
    let manager = JavaAuthManager::from_json_for_application(http.clone(), &probe, application)?;

    match manager.msa_token().await {
        Ok(_) => {
            let session = manager.to_json().await?;
            Ok(RefreshOk { manager, session })
        }
        Err(error) => {
            let retryable = matches!(&error, Error::Msa { status, .. } if status.is_server_error());
            if !retryable {
                return Err(error.into());
            }
            println!("library:  500-class failure: {error:?}");

            // Same form as `msa::refresh_token`; used only to read the body. If
            // it succeeds, the first failure was transient and these tokens win.
            let config: MsaApplicationConfig =
                serde_json::from_value(session["application_config"].clone())?;
            let refresh_token = session["msa_token"]["refresh_token"]
                .as_str()
                .unwrap_or_default();
            let response = http
                .post(config.environment.token_url())
                .form(&[
                    ("client_id", config.client_id.as_str()),
                    ("scope", config.scope.as_str()),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                ])
                .send()
                .await?;
            let status = response.status();
            let body = response.text().await?;

            if status.is_success() {
                let value: serde_json::Value = serde_json::from_str(&body)?;
                let access_token = value["access_token"]
                    .as_str()
                    .ok_or("retry: response has no access_token")?;
                let expires_in = expires_in_seconds(&value)
                    .ok_or("retry: response has no numeric expires_in")?;
                let new_refresh = value
                    .get("refresh_token")
                    .cloned()
                    .unwrap_or_else(|| session["msa_token"]["refresh_token"].clone());
                let mut patched = session.clone();
                patched["msa_token"] = serde_json::json!({
                    "expire_time_ms": now_ms() + expires_in * 1000,
                    "access_token": access_token,
                    "refresh_token": new_refresh,
                });
                let manager = JavaAuthManager::from_json_for_application(
                    http.clone(),
                    &patched,
                    application,
                )?;
                println!(
                    "retry:    {status} — first failure was transient; using the retry's tokens"
                );
                Ok(RefreshOk {
                    manager,
                    session: patched,
                })
            } else {
                println!("retry:    {status} {}", truncate(&body, 600));
                Err(error.into())
            }
        }
    }
}

/// Atomic, owner-only write: a rotated refresh token is as sensitive as a fresh
/// one, and a crash mid-write must not leave a truncated session.
fn save_json(path: &Path, value: &serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
    let body = serde_json::to_vec_pretty(value)?;
    let temp = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
    }
    file.write_all(&body)?;
    file.sync_all()?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

/// Writes the whole cached chain back, so the file holds the newest tokens.
async fn save(manager: &JavaAuthManager, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let json = manager.to_json().await?;
    save_json(path, &json)
}

/// Walks every stage so a failure names the step it died on, then prints the
/// game's arguments.
async fn report(manager: &JavaAuthManager) -> Result<(), Box<dyn std::error::Error>> {
    let token = manager.minecraft_token().await?;
    println!(
        "minecraft: {} token, expires {}",
        token.token_type,
        ms_to_local(token.expire_time_ms)
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

/// One line that says whether the refresh rotated the token, without printing it.
fn summary(old: &serde_json::Value, new: &serde_json::Value) -> String {
    let old_refresh = old["msa_token"]["refresh_token"].as_str().unwrap_or("");
    let new_refresh = new["msa_token"]["refresh_token"].as_str().unwrap_or("");
    let expires = new["msa_token"]["expire_time_ms"].as_i64().unwrap_or(0);
    format!(
        "msa expires {}, refresh token {} (len {}, rotated: {})",
        ms_to_local(expires),
        prefix(new_refresh),
        new_refresh.chars().count(),
        old_refresh != new_refresh
    )
}

fn prefix(token: &str) -> String {
    let head: String = token.chars().take(20).collect();
    if token.chars().count() > 20 {
        format!("{head}…")
    } else {
        head
    }
}

fn expires_in_seconds(value: &serde_json::Value) -> Option<i64> {
    value.get("expires_in").and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn ms_to_local(ms: i64) -> String {
    if ms <= 0 {
        return "-".to_string();
    }
    let time = UNIX_EPOCH + std::time::Duration::from_millis(ms as u64);
    chrono::DateTime::<chrono::Local>::from(time)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn stamp() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

fn truncate(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// Percent-encode one query-string value. The scope's `::` and the redirect's
/// `://` must not be left raw inside an `&`-joined query.
fn query_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The `code` from `…oauth20_desktop.srf?code=…`, or a bare code pasted alone.
fn extract_code(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let raw = match input.split_once("code=") {
        Some((_, after)) => after.split(['&', '#']).next().unwrap_or(after),
        None => input,
    };
    let code = percent_decode(raw.trim());
    (!code.is_empty()).then_some(code)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
