//! Credential redaction for anything MDT prints or returns.
//!
//! Mirror/base URLs are configuration input — a user can paste `https://user:pass@my-mirror/` into
//! `providers.runtime.toml` or a proxy setting — and those URLs end up in error messages, failure
//! reports and debug lines that a launcher writes to its log. [`redact_credentials`] is the single
//! function that keeps the "which file failed" information usable without leaking the credential.
//!
//! It lives here rather than in `customize` because both the reporter plumbing and [`MdtError`]
//! need it; `customize` re-exports it so the historical `mdt::customize::redact_credentials` path
//! keeps working.
//!
//! [`MdtError`]: crate::error::MdtError

// The strict SSRF helpers are only reachable from the (`not(minimal)`) loader-profile path, so a
// `minimal` build legitimately has no caller for them.
#![cfg_attr(feature = "minimal", allow(dead_code))]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether `value` is a 40-character hex SHA-1 digest.
///
/// Used on every hash that arrives from a manifest, an asset index or a profile: an empty or
/// truncated hash silently disables integrity checking, and a hash with path separators in it
/// would escape the content-addressed cache directory.
#[must_use]
pub(crate) fn is_sha1_hex(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `url` is an absolute `http`/`https` URL with a non-empty host.
///
/// This is the *syntactic* check: it says nothing about where the host points. Use it only when the
/// URL comes from trusted configuration (a mirror provider table). For URLs that arrive inside
/// remote metadata — loader profiles, manifests — use [`is_safe_manifest_url`], which also rejects
/// internal addresses.
#[must_use]
pub(crate) fn is_safe_http_url(url: &str) -> bool {
    match split_authority(url) {
        Some((scheme, authority)) => {
            (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
                && host_of(authority).is_some_and(|host| !host.is_empty() && !host.starts_with(':'))
        }
        None => false,
    }
}

/// Whether `url` is an `http`/`https` URL whose host is a **public** address.
///
/// This is the policy for URLs taken from remote metadata (loader profile libraries, manifest
/// entries): those are attacker-controlled, and pointing one at `169.254.169.254` (cloud metadata),
/// `127.0.0.1` (a local admin API) or `10.0.0.5` (the LAN) turns MDT into an SSRF proxy.
///
/// It rejects, in every notation the OS resolver accepts:
///
/// * loopback (`127.0.0.0/8`, `::1`), private (`10/8`, `172.16/12`, `192.168/16`), link-local
///   (`169.254/16` — the metadata service — and `fe80::/10`), CGNAT (`100.64/10`), benchmarking
///   (`198.18/15`), reserved (`240/4`, `0/8`), multicast, unique-local (`fc00::/7`), unspecified,
///   and the `::ffff:` IPv4-mapped form of any of the above;
/// * legacy numeric IPv4 spellings (`2130706433`, `0x7f000001`, `0177.0.0.1`) that libc resolves but
///   `Ipv4Addr::from_str` does not;
/// * local names (`localhost`, `*.localhost`, `*.local`, `*.internal`, `*.home.arpa`).
///
/// Known limit: a *domain name* whose A/AAAA record points at a private address passes here,
/// because checking it needs DNS resolution — and a rebinding attacker could change the answer
/// between the check and the connect. Closing that requires a connect-time guard in the transport,
/// not a URL predicate.
#[must_use]
pub(crate) fn is_safe_manifest_url(url: &str) -> bool {
    if !is_safe_http_url(url) {
        return false;
    }
    let Some((_, authority)) = split_authority(url) else {
        return false;
    };
    let Some(host) = host_of(authority) else {
        return false;
    };
    is_public_host(host)
}

/// Split `scheme://authority…` into `(scheme, authority)`.
fn split_authority(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    Some((scheme, authority))
}

/// The host part of an authority: strips userinfo and port (`u:p@[::1]:8443` → `::1`).
fn host_of(authority: &str) -> Option<&str> {
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(rest) = host_port.strip_prefix('[') {
        return Some(rest.split_once(']').map_or(rest, |(inner, _)| inner));
    }
    match host_port.rsplit_once(':') {
        // A trailing `:digits` is a port; anything else is part of a bare IPv6 literal.
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            Some(host)
        }
        _ => Some(host_port),
    }
}

/// Whether `host` is public: neither an internal IP literal (in any notation) nor a local name.
fn is_public_host(host: &str) -> bool {
    // A trailing dot spells the same host as a fully-qualified name (`169.254.169.254.`), and the
    // resolver accepts it — so normalize before judging.
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        return false;
    }
    if let Some(ip) = parse_host_ip(host) {
        return is_public_ip(ip);
    }
    let lower = host.to_ascii_lowercase();
    let is_local_name = lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
        || lower.ends_with(".home.arpa")
        || lower.ends_with(".localdomain");
    if is_local_name {
        return false;
    }
    // A bare number is an IPv4 in decimal notation to every libc resolver; if we could not parse it
    // as an IP at all, it is still not a plausible DNS name.
    !lower.bytes().all(|b| b.is_ascii_digit())
}

/// Parse `host` as an IP, accepting the legacy numeric IPv4 spellings libc understands.
fn parse_host_ip(host: &str) -> Option<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(ip);
    }
    if let Ok(v6) = host.parse::<Ipv6Addr>() {
        return Some(IpAddr::V6(v6));
    }
    parse_legacy_ipv4(host).map(IpAddr::V4)
}

/// Parse `a` / `a.b` / `a.b.c` / `a.b.c.d`, where each part may be decimal, `0x` hex, or octal
/// (`inet_aton` semantics — the basis of most `127.0.0.1` filter bypasses).
fn parse_legacy_ipv4(host: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }

    let mut values = Vec::with_capacity(4);
    for part in &parts {
        if part.is_empty() {
            return None;
        }
        let value = if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            u32::from_str_radix(hex, 16).ok()?
        } else if part.len() > 1 && part.starts_with('0') {
            u32::from_str_radix(part, 8).ok()?
        } else {
            part.parse::<u32>().ok()?
        };
        values.push(value);
    }

    // A single part is the whole 32-bit address.
    if values.len() == 1 {
        return Some(Ipv4Addr::from(values[0]));
    }

    let (head, tail) = values.split_at(values.len() - 1);
    let mut addr: u32 = 0;
    for value in head {
        if *value > 0xff {
            return None;
        }
        addr = (addr << 8) | value;
    }
    let remaining = 4 - head.len();
    let max = (1u32 << (8 * remaining)) - 1;
    if tail[0] > max {
        return None;
    }
    Some(Ipv4Addr::from((addr << (8 * remaining)) | tail[0]))
}

/// Whether `ip` is a public (routable) address.
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_ipv4(v4),
        IpAddr::V6(v6) => is_public_ipv6(v6),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local() // 169.254.0.0/16, which contains the cloud metadata service
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0 // "this network"
        || (a == 100 && (64..=127).contains(&b)) // CGNAT
        || (a == 198 && (18..=19).contains(&b)) // benchmarking
        || a >= 240) // reserved
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    // `::ffff:127.0.0.1` and friends are the same address in another coat.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_ipv4(v4);
    }
    let segments = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local
        || (segments[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        || segments[..6] == [0, 0, 0, 0, 0, 0]) // ::/96 IPv4-compatible
}

/// Whether `value` can be used as a single path component (an asset-index id, a cache key, ...).
#[cfg(not(feature = "minimal"))]
#[must_use]
pub(crate) fn is_safe_path_component(value: &str) -> bool {
    !value.is_empty() && value != "." && value != ".." && !value.contains(['/', '\\', ':', '\0'])
}

/// Whether `c` ends the authority (host) part of a URL.
///
/// Only these characters can legally end `scheme://host`: a path, query or fragment separator, a
/// quote/closing bracket from the surrounding prose, or whitespace. Trailing punctuation such as a
/// comma or `)` therefore stays attached to the host and is preserved.
fn is_authority_end(c: char) -> bool {
    matches!(c, '/' | '\\' | '?' | '#' | '"' | '\'' | '<' | '>') || c.is_whitespace()
}

/// Replace credentials embedded in URLs with `***`, keeping the host: `scheme://user:pass@host`
/// becomes `scheme://***:***@host`.
///
/// A userinfo without a `:` (`scheme://token@host`) is redacted too. The function is idempotent and
/// only rewrites the authority part, never the path/query: a `?token=` query parameter is part of
/// the resource and cannot be told apart from a legitimate parameter, so it is left untouched.
///
/// # Example
///
/// ```
/// use mdt::redact_credentials;
///
/// assert_eq!(
///     redact_credentials("https://alice:s3cr3t@mirror.example/a.jar"),
///     "https://***:***@mirror.example/a.jar"
/// );
/// assert_eq!(redact_credentials("https://mirror.example/a.jar"), "https://mirror.example/a.jar");
/// ```
#[must_use]
pub fn redact_credentials(input: &str) -> String {
    const SCHEME_END: &str = "://";

    // Fast path: nothing to redact unless the string holds both a scheme and userinfo.
    if !input.contains(SCHEME_END) || !input.contains('@') {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(idx) = rest.find(SCHEME_END) {
        let (head, tail) = rest.split_at(idx + SCHEME_END.len());
        out.push_str(head);

        let end = tail.find(is_authority_end).unwrap_or(tail.len());
        let (authority, after) = tail.split_at(end);
        // `rsplit_once` on the '@': the last one separates userinfo from host, and a password may
        // itself contain an (escaped) '@'.
        match authority.rsplit_once('@') {
            Some((userinfo, host)) if !userinfo.is_empty() => {
                out.push_str(if userinfo.contains(':') {
                    "***:***@"
                } else {
                    "***@"
                });
                out.push_str(host);
            }
            _ => out.push_str(authority),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_url_is_syntactic_only() {
        assert!(is_safe_http_url("https://maven.fabricmc.net/"));
        // The permissive check is for trusted config: a LAN mirror is allowed here.
        assert!(is_safe_http_url("http://192.168.1.5/maven/"));
        assert!(!is_safe_http_url("file:///etc/passwd"));
        assert!(!is_safe_http_url("ftp://example.com/x"));
        assert!(!is_safe_http_url("https:///no-host"));
        assert!(!is_safe_http_url("not a url"));
    }

    #[test]
    fn manifest_url_accepts_public_hosts() {
        for url in [
            "https://maven.fabricmc.net/",
            "https://libraries.minecraft.net/net/x.jar",
            "http://1.1.1.1/x",
            "https://8.8.8.8/x",
            "https://maven.example.com:8443/x",
            "https://[2606:4700:4700::1111]/x",
        ] {
            assert!(is_safe_manifest_url(url), "should be allowed: {url}");
        }
    }

    #[test]
    fn manifest_url_rejects_cloud_metadata_and_private_ipv4() {
        for url in [
            "http://169.254.169.254/latest/meta-data/",
            "http://169.254.169.254./x",
            "http://127.0.0.1:8080/admin",
            "http://127.1/x",
            "https://10.0.0.5/x",
            "https://172.16.4.4/x",
            "https://192.168.1.1/x",
            "https://100.64.0.1/x",
            "https://198.18.0.1/x",
            "https://0.0.0.0/x",
            "https://224.0.0.1/x",
            "https://240.0.0.1/x",
            "https://255.255.255.255/x",
        ] {
            assert!(!is_safe_manifest_url(url), "should be rejected: {url}");
        }
    }

    #[test]
    fn manifest_url_rejects_legacy_numeric_notations() {
        // The notations libc resolves but `Ipv4Addr::from_str` does not, all pointing at 127.0.0.1.
        for url in [
            "http://2130706433/x",
            "http://0x7f000001/x",
            "http://0x7f.0.0.1/x",
            "http://0177.0.0.1/x",
            "http://127.0.1/x",
            "http://99999999999999/x",
        ] {
            assert!(!is_safe_manifest_url(url), "should be rejected: {url}");
        }
        // 2130706433 in hex is 127.0.0.1; 3232235521 is 192.168.0.1.
        assert_eq!(
            parse_legacy_ipv4("2130706433"),
            Some(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            parse_legacy_ipv4("0xC0A80001"),
            Some(Ipv4Addr::new(192, 168, 0, 1))
        );
        assert_eq!(
            parse_legacy_ipv4("3232235521"),
            Some(Ipv4Addr::new(192, 168, 0, 1))
        );
    }

    #[test]
    fn manifest_url_rejects_internal_ipv6() {
        for url in [
            "http://[::1]/x",
            "http://[::]/x",
            "http://[fc00::1]/x",
            "http://[fd12:3456::1]/x",
            "http://[fe80::1]/x",
            "http://[ff02::1]/x",
            "http://[::ffff:127.0.0.1]/x",
            "http://[::ffff:169.254.169.254]/x",
            "http://[::ffff:192.168.0.1]/x",
        ] {
            assert!(!is_safe_manifest_url(url), "should be rejected: {url}");
        }
    }

    #[test]
    fn manifest_url_rejects_local_names() {
        for url in [
            "http://localhost/x",
            "http://app.localhost/x",
            "http://printer.local/x",
            "http://metadata.google.internal/x",
            "http://router.home.arpa/x",
        ] {
            assert!(!is_safe_manifest_url(url), "should be rejected: {url}");
        }
    }

    #[test]
    fn manifest_url_ignores_userinfo_when_judging_the_host() {
        // The credential-looking prefix must not hide a public host…
        assert!(is_safe_manifest_url(
            "https://169.254.169.254:80@mirror.example/x"
        ));
        // …and a public-looking prefix must not smuggle an internal host in.
        assert!(!is_safe_manifest_url("https://mirror.example@127.0.0.1/x"));
    }

    #[test]
    fn manifest_url_rejects_non_http_schemes() {
        for url in [
            "file:///etc/passwd",
            "gopher://127.0.0.1/",
            "data:text/plain,x",
        ] {
            assert!(!is_safe_manifest_url(url), "should be rejected: {url}");
        }
    }
}
