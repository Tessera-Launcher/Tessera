# Development notes

Guidance for working on this crate. Read `README.md` first for what it does and
why it is trimmed.

## Scope

One job: sign in as a chosen application identity (official Minecraft launcher,
or a third-party launcher's Azure application) and hand back the credentials to
start Java Edition — `player_name`, `player_uuid`, `access_token`, `xuid`, plus
the persisted session.

Everything else is out of scope on purpose: webview login, store entitlements,
player certificates, Bedrock, PlayFab, Realms, Xbox profile/gamertag, launcher
UI, version/asset handling. Before adding a feature, check that the game process
actually needs it — the game fetches its own chat certificates, for example.
Do not add per-launcher branches: an identity is data (`AuthApplication`), not
code.

## Commands

```sh
cargo build
cargo test                                   # all tests are inline `#[cfg(test)] mod tests`
cargo test <module>::                        # e.g. `cargo test msa::transport::`
cargo test <name>                            # substring match
cargo fmt
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo doc --no-deps                          # intra-doc links must resolve
```

There is no CI and no `tests/` directory. `fmt`, `clippy` and `doc` are expected
to be clean before a commit; a formatting drift has happened before because
nothing enforced it.

## Style

Keep prose short, in commits and comments alike.

- **Commits:** `type(scope): subject`, lowercase, no trailing period. Types:
  `feat` `fix` `refactor` `chore` `docs`.
- **Comments:** explain what the code cannot say for itself — a protocol quirk,
  a non-obvious constraint (e.g. "Xbox Live sets `X-Err` on 2xx responses").
  Skip narration of what the next line does.

## Architecture

```
src/application.rs   AuthApplication registry, identity matching
src/msa/             Microsoft account: device-code login, refresh, config
src/xbl/             Xbox Live: the two chains, request signing, error codes
src/java/            Minecraft Services: login_with_xbox, profile, launch session
src/{clock,crypto,expirable,error}.rs
```

- `src/msa/config.rs` — `MsaApplicationConfig` (client id, scope, environment,
  chain) plus `validate()`, which refuses to send requests for pairings that
  cannot work. `MsaEnvironment::Live` is for title ids, `MicrosoftOnlineConsumers`
  for Azure applications; `MsaApplicationConfig::new()` defaults to the title
  side, so third-party identities must use `azure_application()`.
- `src/xbl/request.rs` — both chains in one file, split by `XblChain`:
  - `user_authenticate` + `xsts_authorize` (Azure application, `d=` RpsTicket);
  - `device_authenticate` + `sisu_authorize` (title client id, `t=`, signed);
    the latter returns only the XSTS token, the one Java launch consumes.
  `xbl_send`/`classify` decide success from the HTTP status, then read `X-Err`
  or the body's `XErr` only on failure.
- `src/java/manager.rs` — the orchestrator. Each stage is a `Holder<T>`
  (`src/expirable.rs`), a lazily refreshed, expiry-aware cache; `to_json` /
  `from_json` persist the whole chain so a launcher does not re-authenticate.
  `java_xsts_token()` is the single place that resolves the chain to use.

### Multi-identity

A launcher keeps one session per identity. The pieces are:

- `AuthApplication` — label + `MsaApplicationConfig` + the date it was last
  verified. `known_applications()` ships only the official launcher; a
  third-party one is `AuthApplication::azure_application(label, client_id)`.
- `Builder::application(&app)` — sign in as that identity.
- `JavaAuthManager::from_json_for_application(..)` — restore one session,
  refusing it if its client id is not that identity's.
- `JavaAuthManager::restore_first_working(..)` — restore the first candidate
  whose identity matches *and* whose refresh succeeds, reporting every skipped
  candidate in `Error::NoUsableSession`. This is the startup path: it refreshes,
  so the caller must persist the returned session.
- `Error::needs_sign_in()` — distinguishes "ask the user to sign in again" from
  "retry".

### Invariants — do not break these

1. **No request for an impossible identity.** `MsaApplicationConfig::validate()`
   runs at every network entry point and at `Builder::login_msa_token` /
   `from_json`. Adding a new endpoint means adding the call.
2. **HTTP status decides, then `X-Err`.** Xbox Live sets `X-Err` on 2xx
   responses too; checking it first turned working logins into errors.
3. **`Accept: application/json` on every Xbox Live call.** MinecraftAuth gets it
   from a shared client; we take the caller's client, so it is set per request.
4. **Errors never carry tokens, refresh tokens, auth codes or private keys.**
   Only status, code, name, message.
5. **Chain selection is explicit.** Never infer it from the client id alone at
   login time; `MsaApplicationConfig.chain` may be `None` only for saves written
   before the field existed.
6. **A saved session is bound to its client id.** Refresh tokens are issued per
   application, so `from_json_for_application` refuses a mismatched identity.
7. **Clock offset is sampled, not cached forever, and never caches a failure.**
   A pinned offset of 0 breaks every signed request for the process lifetime.
8. **`reqwest` keeps a TLS feature.** With `default-features = false` and no
   TLS feature, every `https://` request fails inside hyper-util with
   `invalid URL, scheme is not http` — the message looks like a network fault,
   so this is documented here and guarded by the `#[ignore]`d test
   `msa::request::tests::https_requests_reach_microsoft`
   (`cargo test -- --ignored`).
9. **No proxy feature on purpose.** `system-proxy` is off, so `*_PROXY` env vars
   are ignored: with the `socks` feature absent, a socks proxy would resurface
   the same confusing error. Callers who need a proxy pass their own
   `reqwest::Client` with an explicit `Proxy`.

## Verification status

| Path | Status |
|---|---|
| Azure application identity, `consumers`, device code | **verified** 2026-11-07 (Prism Launcher's client id), re-checked after the trimming refactor: sign-in, refresh, and a refresh of the rotated token |
| Azure application identity on `login.live.com` | device code issued, rest unverified — allowed, not blocked |
| Title client id on `consumers` | known to fail (`AADSTS700016`), rejected before any request |
| Official launcher identity (title id + device + SISU), sign-in | **verified** 2026-11-07: device code → SISU → `login_with_xbox` → profile all succeeded against a real account |
| Official launcher identity, MSA refresh | **verified** 2026-11-07 on a second account: a freshly issued token refreshes, and the rotated token refreshes again |
| Official launcher identity, online-mode multiplayer | **verified** 2026-11-08 (reported by the launcher project: a real client signed in with the title identity joined an online-mode server) |
| Third-party identity, online-mode multiplayer | unverified — same test, not yet run with an Azure application id |

Which identity a Minecraft access token belongs to is visible in its `aid` claim:
`…402b5328` for the official launcher, the Azure application id for a third-party
identity.

Online-mode multiplayer was confirmed with the official launcher identity on
2026-11-08: the token is accepted by a real server, not just by single-player.

### Refresh behaviour, and one misleading error

Refreshing a title-identity session works: `login.live.com/oauth20_token.srf`
returns a new access token and a rotated refresh token, for both the token
issued by the sign-in and the one issued by a refresh. The example exercises
both paths on every run.

One old token did not, though, and it is worth remembering how it failed. The
request was byte-for-byte what MinecraftAuth sends, reproduced with two
independent HTTP clients, POST and GET, with and without `redirect_uri`, with
`client_info`, with MSAL headers, and with every plausible scope — and
`login.live.com` answered:

```
500 {"error":"server_error","foci":"1","correlation_id":"…"}
```

Calibration pinned it on the token, not the request: a garbage or truncated
token, and the same token with the wrong client id, all got clean
`400 invalid_grant` responses ("The token was issued for a different client
id"), while the real token passed that binding check and then died inside
Microsoft's FOCI redemption. So a token can be dead in a way that surfaces as
`500 server_error` instead of `invalid_grant`.

Consequence for callers: `Error::needs_sign_in()` classifies `invalid_grant` and
friends, and deliberately not `server_error` — but a 5xx from the token endpoint
should be retried once and then treated as "sign in again", never retried
forever.

### How to verify the official launcher chain

The wire shapes match the Java reference implementation field by field and
`login.live.com/oauth20_connect.srf` does issue a device code for
`00000000402b5328`, so what is untested is everything after the user signs in:
the signed `device/authenticate`, SISU, `login_with_xbox`, and refresh.

1. `cargo run --example verify_login` (no arguments = official launcher;
   `--client-id <uuid>` = a third-party Azure application) — signs in with
   `login_device_code`, prints the stage that failed, the session, the game's
   launch arguments, and a refresh canary. The example is a verification aid
   only: it is not library API and is not tested.
2. Complete the sign-in in a browser with an account that owns Java Edition.
   A failure at this stage surfaces as `Error::Xbl` (with Xbox's own
   `Message`/`Redirect` folded in) or `Error::Msa` — the variant names the stage.
3. Check the result: `xuid` should be present (it is read from the Minecraft
   access token's own claims — Xbox Live's Java-relying-party responses do not
   carry it), and `player_name`/`player_uuid` must match the account.
   `Error::ProfileNotFound` means authentication worked but the account has no
   Minecraft profile.
4. **Prove the token, not just the process.** An invalid access token still
   starts the game in single-player; it only fails when joining a server. This
   step is done for the official launcher identity (2026-11-08) and still
   outstanding for a third-party identity. So launch a real client with
   `--username/--uuid/--accessToken/--userType msa/--xuid` and join an online-mode
   server (or a Realm). That is the only end-to-end evidence that matters.
5. Then record the date: `AuthApplication::official_launcher().verified("YYYY-MM-DD")`
   in `known_applications()`, which is what the `verified` field is for.

The refresh check reuses the saved session (`session.json`, git-ignored because
it holds live tokens) with the MSA expiry zeroed, so it exercises `refresh_token`
in seconds instead of hours:

```sh
cargo run --example verify_login -- --resume
```

The example persists the session after every successful refresh because the
refresh token rotates; a real launcher has to do the same — this crate only
produces tokens, storing them is the caller's job.

If step 2 fails, compare against `RaphiMC/MinecraftAuth`'s
`XblDeviceAuthenticateRequest` / `XblSisuAuthorizeRequest` (the request bodies
and `x-xbl-contract-version` are the first things to check) before touching the
signing code — `src/xbl/sign.rs` and `src/crypto.rs` are covered by unit tests.

## Syncing with upstream

The git remote `upstream` is `CCBlueX/minecraft-auth-rs`, and the vendoring
commit has upstream's `68e4deb` as its parent, so the local delta is just:

```sh
git fetch upstream --tags
git log --oneline upstream/main..HEAD     # our changes
git diff upstream/main                    # full delta
```

Most of that delta is intentional (see README's Local changes): deleted webview
transport, deleted entitlements/certificates, removed `MicrosoftOnlineCommon`,
the non-title chain, explicit `XblChain`, `validate()`, `AuthApplication`, the
clock fix, and the `X-Err`/status ordering. Do not "restore" those while merging.

For wire behaviour, also diff the Java reference from the baseline commit named
in the README's Upstream table:

```sh
git clone https://github.com/RaphiMC/MinecraftAuth.git /tmp/minecraftauth
cd /tmp/minecraftauth && git log --oneline <baseline>..HEAD -- src/main/java/net/raphimc/minecraftauth
```

An empty log means nothing in scope changed. When upstream tags a release, merge
`upstream/main` first, then re-check the wire shapes and bump the commit hash in
the README.
