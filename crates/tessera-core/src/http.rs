//! One place that builds HTTP clients.
//!
//! Four call sites used to build their own: the GUI's login client, the token
//! refresh, the Adoptium client and (inside the vendored crates) whatever MDT and
//! `minecraft-auth` do for themselves. Each had its own user agent, timeouts and
//! TLS setup, so a policy change — a proxy, a longer connect timeout, a retry —
//! had to be found and applied several times, and the ones that were missed were
//! invisible until a network problem showed up.
//!
//! The vendored crates keep their own clients (they are separate workspaces with
//! their own configuration surfaces); everything this crate and its front-end
//! talk to goes through here.

use std::time::Duration;

use crate::error::{Error, Result};

/// Adoptium's API sits behind Cloudflare and answers 403 to clients that look
/// like a script, so the agent is not optional.
const USER_AGENT: &str = concat!("tessera-launcher/", env!("CARGO_PKG_VERSION"));

/// A stalled login poll or version-list request must not hold a worker forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// The client used by login and token refresh.
pub fn async_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        .map_err(|error| Error::internal(format!("无法创建 HTTP 客户端：{error}")))
}

/// The client used by the Java runtime downloader.
///
/// `total` caps the whole artifact download, which is why it is a parameter: a
/// metadata request wants seconds, a 200 MB runtime wants minutes.
pub fn blocking_client(total: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(total)
        .build()
        .map_err(|error| Error::internal(format!("无法创建 HTTP 客户端：{error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_clients_build() {
        assert!(async_client().is_ok());
        assert!(blocking_client(Duration::from_secs(60)).is_ok());
    }
}
