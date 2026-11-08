//! Shared HTTP client construction.

use std::time::Duration;

use crate::error::MdtError;

/// A Chrome-like `User-Agent`, used to avoid being blocked by mirror CDNs and WAFs that
/// reject requesters without a browser UA.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// Build the shared HTTP client used by [`crate::MdtClient`].
///
/// Cheap to clone (`Arc` under the hood) and reuses one connection pool across all downloads,
/// so you're not doing a TLS handshake per file.
///
/// The keep-alive pool is deliberately generous (90s idle) so a burst of asset downloads reuses
/// warm connections. Its per-host ceiling is derived from the caller's asset concurrency: a pool
/// smaller than the concurrency thrashes connections exactly when the extra parallelism was
/// supposed to help, so the pool is `asset_concurrency + 4` with a floor of 20.
///
/// # HTTP/1.1 by default, and why
///
/// reqwest negotiates HTTP/2 via ALPN when the server offers it. For Minecraft assets that is the
/// slower choice: every stream lands on one connection, and the Gemalto/Azure CDN in front of
/// `resources.download.minecraft.net` serves that arrangement at about half the throughput of a
/// pool of HTTP/1.1 connections. Measured on 1.8.9's 722 asset objects (109.5 MiB) from one
/// machine, same network, warmed page cache:
///
/// | transport | concurrency | assets phase |
/// |---|---|---|
/// | HTTP/2 (multiplexed) | 16 | 46.7 s |
/// | HTTP/1.1 (pooled) | 16 | 29.5 s |
/// | HTTP/2 (multiplexed) | 64 | 32.2 s |
/// | HTTP/2 + adaptive window | 64 | 30.2 s |
/// | HTTP/1.1 (pooled) | 64 | 15.3 s |
/// | `curl`, one connection per object | 64 | 19.9 s |
///
/// So HTTP/1.1 is the default; [`crate::MdtClientBuilder::http1_only`] turns multiplexing back
/// on for a mirror that demonstrably prefers it.
///
/// Timeouts protect against a stalled mirror without killing slow-but-healthy downloads: the
/// connect phase is capped by `connect_timeout`, and `read_timeout` aborts only when no bytes
/// arrive for that long (it resets after every successful read). There is deliberately no *total*
/// request timeout, since large files legitimately take longer than any fixed wall-clock bound.
pub fn build_client(
    proxy: Option<&str>,
    connect_timeout: Duration,
    read_timeout: Duration,
    asset_concurrency: usize,
    http1_only: bool,
) -> Result<reqwest::Client, MdtError> {
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(asset_concurrency.saturating_add(4).max(20))
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout);

    if http1_only {
        builder = builder.http1_only();
    } else {
        // The h2 path is the slower one for assets; when it is requested, at least let the window
        // grow to the BDP instead of staying at the 64 KiB default (30.2 s vs 32.2 s measured).
        //
        // This crate no longer enables reqwest's `http2` feature by default (VENDORED.md section
        // 8), so the method only exists when the `http2` feature is opted into. Without it, ALPN
        // never offers h2 and `http1_only(false)` degrades to the same transport as
        // `http1_only(true)`.
        #[cfg(feature = "http2")]
        {
            builder = builder.http2_adaptive_window(true);
        }
    }

    if let Some(proxy) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }

    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_client_succeeds() {
        let _client = build_client(
            None,
            Duration::from_secs(15),
            Duration::from_secs(30),
            16,
            true,
        )
        .unwrap();
    }

    #[test]
    fn build_client_succeeds_with_proxy() {
        let _client = build_client(
            Some("http://127.0.0.1:1"),
            Duration::from_secs(15),
            Duration::from_secs(30),
            16,
            true,
        )
        .unwrap();
    }

    #[test]
    fn build_client_accepts_multiplexing_request_even_without_http2() {
        // `http1_only(false)` must stay compilable when the `http2` feature is
        // off: ALPN then never offers h2 and the flag is a no-op.
        let _client = build_client(
            None,
            Duration::from_secs(15),
            Duration::from_secs(30),
            16,
            false,
        )
        .unwrap();
    }

    #[test]
    fn user_agent_is_browser_like() {
        assert!(USER_AGENT.contains("Mozilla"), "missing Mozilla token");
        assert!(USER_AGENT.contains("Chrome"), "missing Chrome token");
    }
}
