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

/// Normalise what the user typed into a proxy base.
///
/// `gh-proxy.com`, `https://gh-proxy.com/` and `http://127.0.0.1:8080` all work;
/// anything without a scheme gets `https://`.
pub fn normalize_proxy(input: &str) -> Option<String> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    })
}

/// Every URL to try for a `github.com` release asset, in order.
///
/// The direct link comes last: a proxy that is down is a slow download rather
/// than a dead end. Both spellings proxies expect are offered — `proxy/host/…`
/// and `proxy/https://host/…` — because the two are not interchangeable.
pub fn github_candidates(url: &str, proxy: &str) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(proxy) = normalize_proxy(proxy) {
        let bare = url.split_once("://").map_or(url, |(_, rest)| rest);
        urls.push(format!("{proxy}/{bare}"));
        urls.push(format!("{proxy}/{url}"));
    }
    urls.push(url.to_string());
    urls.dedup();
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_clients_build() {
        assert!(async_client().is_ok());
        assert!(blocking_client(Duration::from_secs(60)).is_ok());
    }

    #[test]
    fn a_proxy_goes_in_front_of_a_github_url_both_ways() {
        let url = "https://github.com/a/b/releases/download/v1/x.zip";
        assert_eq!(github_candidates(url, "  "), [url]);
        let proxied = github_candidates(url, "gh-proxy.com");
        assert_eq!(proxied.len(), 3, "{proxied:?}");
        assert_eq!(
            proxied[0],
            "https://gh-proxy.com/github.com/a/b/releases/download/v1/x.zip"
        );
        assert_eq!(proxied[1], format!("https://gh-proxy.com/{url}"));
        assert_eq!(proxied[2], url);
    }
}
