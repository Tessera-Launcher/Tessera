//! `redact_credentials` is the crate's only defence against a mirror URL with an embedded
//! password reaching a launcher's log, so it gets its own integration suite rather than a corner
//! of `customize.rs`.
//!
//! Every case is checked three ways: the exact expected string, a "no secret survives" assertion,
//! and idempotence (re-running the redaction must not corrupt an already-redacted line — launcher
//! logs get re-formatted, re-serialized and re-printed).
//!
//! Scope reminder: only the URL *userinfo* (`scheme://user:pass@host`) is a credential. A query
//! parameter such as `?token=…` is part of the resource and is deliberately left alone.

use std::time::Instant;

use mdt::redact_credentials;

/// URL shapes a mirror config (or an error message carrying one) can realistically contain.
const NASTY_URLS: &[(&str, &str)] = &[
    // The plain case, with a port, query and fragment.
    (
        "https://user:pass@host:8080/path?query=foo#frag",
        "https://***:***@host:8080/path?query=foo#frag",
    ),
    (
        "http://alice:s3cr3t@mirror.example/a.jar",
        "http://***:***@mirror.example/a.jar",
    ),
    // Scheme case is irrelevant: "://" is the anchor, not the scheme name.
    (
        "HTTPS://Alice:Pass@Mirror.Example/Path",
        "HTTPS://***:***@Mirror.Example/Path",
    ),
    // Non-HTTP schemes reach the same code path (a proxy or a custom provider).
    ("wss://u:p@host/socket", "wss://***:***@host/socket"),
    (
        "socks5://u:p@127.0.0.1:1080",
        "socks5://***:***@127.0.0.1:1080",
    ),
    // A token without a colon is still a credential.
    ("https://token@host/x", "https://***@host/x"),
    // No path at all: the authority runs to the end of the string.
    ("https://u:p@host", "https://***:***@host"),
    // IPv6 literal: the colons inside the brackets must survive.
    ("https://u:p@[::1]:8443/x", "https://***:***@[::1]:8443/x"),
    // Several '@': the last one separates userinfo from host.
    ("https://a@b@host/x", "https://***@host/x"),
    // Empty password / empty user.
    ("https://user:@host/x", "https://***:***@host/x"),
    ("https://:pass@host/x", "https://***:***@host/x"),
    // Percent-encoded '@' inside the password.
    ("https://u:p%40ss@host/x", "https://***:***@host/x"),
    // Embedded in JSON, quotes included.
    (
        r#"{"url":"https://u:p@h/x"}"#,
        r#"{"url":"https://***:***@h/x"}"#,
    ),
    // Two URLs on one line, different credentials.
    (
        "a https://u:p@h1/x b https://u2:p2@h2/y",
        "a https://***:***@h1/x b https://***:***@h2/y",
    ),
    // Surrounding prose punctuation is not part of the authority.
    (
        "failed: (https://u:p@h/x), retrying",
        "failed: (https://***:***@h/x), retrying",
    ),
    // A credential hidden in a nested redirect URL.
    (
        "https://u:p@h/x?to=ftp://a:b@c/y",
        "https://***:***@h/x?to=ftp://***:***@c/y",
    ),
    // Non-ASCII user and host.
    (
        "下载失败 https://用户:口令@镜像.example/文件.jar 重试中",
        "下载失败 https://***:***@镜像.example/文件.jar 重试中",
    ),
];

/// Text that must pass through untouched (a false positive here mangles a real URL).
const INNOCENT_TEXT: &[&str] = &[
    "https://mirror.example/a.jar",
    // An '@' in the *path* is not userinfo.
    "https://host/a@b",
    "file:///C:/Users/alice@corp/x",
    // No scheme at all.
    "mailto:alice@example.com",
    "user:pass@host without a scheme",
    r"C:\Users\alice\AppData\Roaming\.minecraft",
    "libraries/net/minecraft/client.jar",
    "no url here at all",
    "",
];

#[test]
fn redacts_every_credential_shape() {
    for (input, expected) in NASTY_URLS {
        assert_eq!(redact_credentials(input), *expected, "input: {input}");
    }
}

#[test]
fn never_leaks_a_credential() {
    // Negative test: whatever the shape, no user and no password may survive, in any output.
    let secrets = [
        "alice", "s3cr3t", "hunter2", "pass", "p%40ss", "token", "口令", "用户",
    ];
    for (input, _) in NASTY_URLS {
        let redacted = redact_credentials(input);
        for secret in secrets {
            // `pass`/`token` show up in `?query=foo`-style text? Not here: these inputs are chosen
            // so the only occurrence of each secret is inside the userinfo.
            assert!(
                !redacted.contains(secret),
                "{secret:?} survived in {redacted:?} (from {input:?})"
            );
        }
        assert!(redacted.contains("***"), "nothing was redacted: {input:?}");
    }
}

#[test]
fn never_leaks_a_credential_inside_an_error_line() {
    // Mirrors how the crate itself emits failure text: a URL glued into prose.
    let line =
        "network error while fetching https://alice:hunter2@mirror.example/a.jar: 404 Not Found";
    let redacted = redact_credentials(line);
    assert!(
        !redacted.contains("alice") && !redacted.contains("hunter2"),
        "{redacted}"
    );
    assert!(redacted.ends_with("a.jar: 404 Not Found"), "{redacted}");
}

#[test]
fn leaves_innocent_text_alone() {
    for input in INNOCENT_TEXT {
        assert_eq!(redact_credentials(input), *input, "input: {input}");
    }
    // An empty userinfo is not a credential, and inventing `***` there would corrupt the URL.
    assert_eq!(redact_credentials("https://@host/x"), "https://@host/x");
}

#[test]
fn is_idempotent() {
    for input in NASTY_URLS
        .iter()
        .map(|(input, _)| *input)
        .chain(INNOCENT_TEXT.iter().copied())
        .chain(["https://@host/x", "https://u:p@", "://@", "https://@@"])
    {
        let once = redact_credentials(input);
        assert_eq!(
            redact_credentials(&once),
            once,
            "not idempotent for {input:?}"
        );
    }
}

#[test]
fn survives_adversarial_input() {
    // Degenerate shapes must not panic and must not invent credentials where there are none.
    for input in [
        "://",
        "@",
        "https://@",
        "://@",
        "https://:@/",
        "https://@@",
        "https://u:p@",
        "a://b@c@d@e",
        "https://u:p@h\\x",
        "https:///a@b",
        "https://\u{0}@host/x",
    ] {
        let _ = redact_credentials(input); // must not panic
    }
    assert_eq!(redact_credentials("https://u:p@"), "https://***:***@");
    assert_eq!(redact_credentials("a://b@c@d@e"), "a://***@e");
}

#[test]
fn scales_linearly_on_a_large_log_line() {
    // 20k URLs in one line: a quadratic implementation would take minutes here.
    let one = "https://alice:hunter2@mirror.example/asset.jar ";
    let input = one.repeat(20_000);

    let started = Instant::now();
    let redacted = redact_credentials(&input);
    let elapsed = started.elapsed();

    assert!(
        !redacted.contains("hunter2"),
        "leaked a credential at scale"
    );
    assert_eq!(redacted.matches("***:***@").count(), 20_000);
    assert!(
        elapsed.as_secs() < 5,
        "redaction took {elapsed:?} for a 20k-URL line, which suggests non-linear behaviour"
    );
}
