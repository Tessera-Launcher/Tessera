# minecraft-auth

Microsoft → Xbox Live → Minecraft Services authentication for Java Edition,
trimmed for launchers: sign in as one application identity and get the
credentials to start the game.

Vendored from **[CCBlueX/minecraft-auth-rs](https://github.com/CCBlueX/minecraft-auth-rs)**
(v0.1.1, commit `68e4deb`), itself a port of
[RaphiMC/MinecraftAuth](https://github.com/RaphiMC/MinecraftAuth). The two Xbox
Live chains, the device-code transport and the error handling come from there;
this copy drops everything a launcher does not need and adds the non-title
chain, explicit chain selection and the identity registry. See
[Local changes](#local-changes) for the full list, and [AGENTS.md](AGENTS.md)
for development notes.

Licensed LGPL-3.0-or-later, inherited from upstream — see [LICENSE](LICENSE).

## What it does

- Device-code login — no browser, no redirect URI, so it works for any identity
- Two Xbox Live chains: title client id (official launcher, device token + SISU)
  and Azure application client id (third-party launchers, user + XSTS)
- Launcher identities as data (`AuthApplication`): the official launcher plus
  any third-party Azure application, with a check that a saved session belongs
  to the identity it is being restored for, and one call that picks the first
  saved session that still refreshes
- Lazy token refresh per stage, and whole-session JSON persistence
- `--username`, `--uuid`, `--accessToken` and `--xuid` from one call

Deliberately absent: webview/authorization-code login, store entitlements,
player certificates, Bedrock Edition, PlayFab, Realms, Xbox profile. A launcher
needs none of them to start the game.

## Install

```sh
cargo add minecraft-auth
```

## Usage

### Getting started

Bring your own `reqwest::Client`:

```rust
let http_client = reqwest::Client::new();
```

### Configure the auth manager

```rust
use minecraft_auth::java::JavaAuthManager;

let builder = JavaAuthManager::builder(http_client);
```

### Logging in as an identity

```rust
use minecraft_auth::{AuthApplication, java::JavaAuthManager};

let application = AuthApplication::official_launcher(); // or your own Azure app id

let auth_manager = JavaAuthManager::builder(http_client)
    .application(&application)
    .login_device_code(|code| {
        println!("go to {}", code.direct_verification_uri());
        println!("enter code {}", code.user_code);
    })
    .await?;
```

A third-party identity is just a config:

```rust
let application = AuthApplication::new(
    "Prism Launcher",
    MsaApplicationConfig::azure_application(
        "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb",
        constants::SCOPE_OFFLINE_ACCESS,
    ),
);
```

### Saving and loading sessions

```rust
let saved = auth_manager.to_json().await?;

// Refuses a session that was signed in as a different application.
let auth_manager = JavaAuthManager::from_json_for_application(http_client, &saved, &application)?;
```

### Launching the game

```rust
let session = auth_manager.launch_session().await?;
// session.player_name  -> --username
// session.player_uuid  -> --uuid
// session.access_token -> --accessToken
// session.xuid         -> --xuid (optional)
```

## Verifying the official launcher identity

Sign-in and refresh with the title client id both work end to end (device code
→ SISU → `login_with_xbox` → profile, and a forced refresh of the saved
session), verified 2026-11-07 on two accounts. Still unverified: a real client
joining an online-mode server with the resulting token. See
[AGENTS.md](AGENTS.md) for what was checked and how.
`examples/verify_login.rs` is the tool for that — it is a verification aid, not
part of the library API, and it talks to the real services. It verifies any
identity, including a third-party one:

```sh
cargo run --example verify_login                                        # official launcher
cargo run --example verify_login -- --client-id <azure-client-id>       # third-party app
cargo run --example verify_login -- --resume                            # refresh a saved session
```

It prints the stage that failed and the game's launch arguments, checks refresh
right after sign-in, and saves `session.json` (git-ignored; contains live tokens
— delete it after).
Passing it still does not prove the session: an invalid token starts the game in
single-player, so launch a real client and join an online-mode server. Once that
works, record the date in `known_applications()` via `.verified("YYYY-MM-DD")`.
Details in [AGENTS.md](AGENTS.md).

## Upstream

| | |
|---|---|
| Derived from | [CCBlueX/minecraft-auth-rs](https://github.com/CCBlueX/minecraft-auth-rs) @ [`68e4deb`](https://github.com/CCBlueX/minecraft-auth-rs/commit/68e4deb9b98940d2f37f7339cc48c200163d1e4f) (tag `0.1.1`) — the git remote `upstream` points here |
| Wire protocol checked against | [RaphiMC/MinecraftAuth](https://github.com/RaphiMC/MinecraftAuth) @ [`654174d`](https://github.com/RaphiMC/MinecraftAuth/commit/654174dd1f5bc27c93cf4f3bb2961664845d539c) (`5.0.3-SNAPSHOT`) |
| Ported | Java Edition login (MSA → XBL device token → XBL SISU authorize → Minecraft Services), the non-title user + XSTS chain, device-code transport |
| Not ported | Webview/authorization-code transport, store entitlements, player certificates, Bedrock Edition, PlayFab, Realms, Xbox Live profile/gamertag |

## Local changes

What this copy does differently from upstream:

- **Non-title (3-leg) XBL auth added** (`src/xbl/request.rs` + `src/java/manager.rs`).
  `JavaAuthManager` now picks its chain from the application config: a title
  client id uses the device token + SISU call, a regular Azure application
  (dashed UUID, e.g. Prism Launcher's) uses `user/authenticate` with a `d=<
  RpsTicket>` followed by a standalone `xsts/authorize`. Title client ids keep
  upstream's `t=` prefix behaviour.
- **v2 endpoints actually work**: `device_code` grant type becomes the
  RFC 8628 URN for the `login.microsoftonline.com` environments, and
  `authorization_pending`/`slow_down` are handled per RFC 8628. Upstream only
  ships the v1 `login.live.com` environment, where neither matters.
- **`Accept: application/json` is set on every Xbox Live request.** MinecraftAuth
  gets this from its shared HTTP client (`MinecraftAuth.createHttpClient()`); this
  port takes a caller-supplied `reqwest::Client` instead, so the calls went out
  with reqwest's default `Accept: */*`, which the endpoints document that they
  reject.
- **SISU's user and title tokens are not parsed.** The call returns three
  tokens; Java Edition launch only consumes the XSTS one, so the other two are
  left out of the response type instead of being carried around unused.
- **Trimmed to what a launcher needs.** Upstream's webview transport (config
  `client_secret`/`redirect_uri`, `auth_code_url`, `exchange_auth_code`), store
  entitlements, player certificates and Xbox profile support are deleted rather
  than feature-gated — a launcher passes none of them to the game, and the game
  fetches its own certificates. `MsaEnvironment::MicrosoftOnlineCommon` is gone
  too: `XboxLive.signin` only works in the `consumers` tenant.
- **`X-Err` is no longer trusted on a successful response.** Xbox Live also sets
  the header on 2xx responses — a real `user/authenticate` `200` carrying a valid
  token came back with `X-Err: 5` — so reading it before the status turned a
  working login into `XERR_0x00000005`. The HTTP status now decides whether the
  code means anything.
- **The XBL chain is chosen explicitly.** `MsaApplicationConfig` carries an
  `Option<XblChain>` (`with_chain`), guessed from the client id's shape at
  construction and re-inferred for saves written before the field existed. A
  title id paired with a `login.microsoftonline.com` environment is rejected by
  `validate()` before any request is sent (that pairing answers
  `AADSTS700016`); the reverse pairing is deliberately allowed, since
  `login.live.com` does issue device codes for Azure application ids.
- **Clock offset is re-sampled instead of cached forever.** The offset used for
  XBL request signing expires after 30 minutes, and a failed sample is never
  cached — a single network blip during the old one-shot sample used to pin the
  offset at 0 for the whole process, breaking every signed request.
- **Launcher identities are data.** `AuthApplication` bundles a label, a
  `MsaApplicationConfig` and the date it was last verified; `known_applications()`
  ships only the official launcher, because every other client id belongs to
  someone else's Azure application. `Builder::application()` picks one, and
  `JavaAuthManager::from_json_for_application` refuses a saved session whose
  client id is not that identity's — a refresh token is only usable by the
  client id it was issued to.
- **`JavaLaunchSession` carries the XUID.** `xsts/authorize` answers with
  `DisplayClaims.xui[0].xid`; it is parsed into the XSTS token and exposed as
  the game's optional `--xuid` argument.
- **Device-code polling tells the caller what happened.** `expired_token` now
  surfaces as `DeviceCodeExpired` instead of a generic timeout, and the login
  link prefers the server's own `verification_uri_complete` when it sends one.
- **Xbox error bodies are no longer discarded.** `XErr` is read from either the
  `X-Err` header or the body, and the body's `Message`/`Redirect` (which is what
  actually tells the user what to do — a family page, a proof-of-age page, ...)
  is folded into the error message. Previously any non-JSON body became a bare
  `failed to parse response body`.
- `[workspace]` was added so the crate is not pulled into a caller's workspace.

## Commits

`type(scope): subject`, lowercase, no trailing period.

`feat` `fix` `refactor` `chore` `docs`

## License

LGPL-3.0-or-later. See [LICENSE](LICENSE).
