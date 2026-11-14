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
