//! Microsoft → Xbox Live → Minecraft Services authentication for Java Edition.
//!
//! This is a vendored, trimmed copy of <https://github.com/CCBlueX/minecraft-auth-rs>
//! (itself a port of RaphiMC/MinecraftAuth) built for one job: sign in as a
//! chosen *application identity* — the official Minecraft launcher's client id,
//! or a third-party launcher's Azure application — and hand back the credentials
//! a launcher needs to start the game.
//!
//! Two Xbox Live chains exist and both are compiled in, because which one
//! applies depends on the identity at runtime:
//!
//! - a title client id (official launcher) uses the proof-of-possession device
//!   token plus SISU `/authorize`;
//! - a regular Azure application client id uses `user/authenticate` followed by
//!   `xsts/authorize`.
//!
//! Login is device-code only: it needs no redirect URI, so it works for any
//! identity without per-application browser plumbing.
//!
//! Deliberately absent: Bedrock Edition, PlayFab, Realms, Xbox profile, store
//! entitlements, player certificates, and the webview login flow. None of those
//! produce anything a launcher passes to the game.

mod application;
mod clock;
mod crypto;
mod error;
mod expirable;

/// Applied to every request this crate makes, on top of whatever the caller
/// configured on the `reqwest::Client` it handed us. All of these endpoints
/// exchange small JSON payloads, so a request still in flight after this
/// long is stuck, not slow — and without a bound it would hang the caller
/// forever instead of surfacing an error it can show or retry.
pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub mod java;
pub mod msa;
mod xbl;

pub use application::{known_applications, AuthApplication};
pub use crypto::EcKeyPair;
pub use error::{Error, Result};
pub use expirable::Expirable;
