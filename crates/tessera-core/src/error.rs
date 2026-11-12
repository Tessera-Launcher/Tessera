//! The core's error type.
//!
//! Workers report user-facing strings, but the code between a failed syscall and
//! that string used to be `String` all the way down. That meant nothing could
//! tell a network hiccup from a full disk, and tests had to match on prose.
//!
//! [`Error`] keeps the message — the UI still shows it — and adds the category a
//! caller needs to decide whether retrying can help. Constructors are the only
//! way to build one, so the category and the message cannot drift apart.

use std::error::Error as StdError;
use std::fmt;
use std::io;

/// What kind of failure this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A filesystem or process-spawn syscall failed.
    Io,
    /// A request could not be completed: DNS, TLS, timeout, HTTP status.
    Network,
    /// Bytes arrived but did not match the expected hash or size.
    Integrity,
    /// Something the operation needs is not on disk.
    Missing,
    /// A file exists but its content is not valid for this operation.
    Invalid,
    /// A child process failed, timed out or printed the wrong thing.
    Process,
    /// The launcher itself is in a state it should not be in.
    Internal,
    /// Another process holds a resource this one needs (the instance lock).
    Busy,
}

/// One failure, with a user-facing message and a machine-readable category.
#[derive(Debug)]
pub struct Error {
    kind: Kind,
    message: String,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl Error {
    fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    fn with_source(
        kind: Kind,
        message: impl Into<String>,
        source: Box<dyn StdError + Send + Sync>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            source: Some(source),
        }
    }

    /// A syscall failed. `message` says what was being attempted, without
    /// repeating the OS error (the `Display` impl appends it).
    pub fn io(message: impl Into<String>, source: io::Error) -> Self {
        Self::with_source(Kind::Io, message, Box::new(source))
    }

    /// A request failed or came back unusable.
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(Kind::Network, message)
    }

    /// The bytes arrived but were not what they claimed to be.
    pub fn integrity(message: impl Into<String>) -> Self {
        Self::new(Kind::Integrity, message)
    }

    /// A required file, directory or runtime is absent.
    pub fn missing(message: impl Into<String>) -> Self {
        Self::new(Kind::Missing, message)
    }

    /// A file or response exists but cannot be understood.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(Kind::Invalid, message)
    }

    /// A child process failed, timed out, or printed the wrong thing.
    pub fn process(message: impl Into<String>) -> Self {
        Self::new(Kind::Process, message)
    }

    /// A bug or an impossible state; never the user's fault.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Kind::Internal, message)
    }

    /// Another process holds what this one needs.
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(Kind::Busy, message)
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The underlying syscall failure, when there is one.
    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        self.source
            .as_ref()
            .and_then(|source| source.downcast_ref::<io::Error>())
            .map(io::Error::kind)
    }

    /// The message a worker sends to the UI.
    ///
    /// Transient failures get an explicit hint, because the UI has no retry
    /// button: the user has to know that pressing the key again is reasonable.
    pub fn user_message(&self) -> String {
        if self.is_transient() {
            format!("{self}（网络或磁盘暂时不可用，可以重试）")
        } else {
            self.to_string()
        }
    }

    /// Whether trying the same operation again could plausibly succeed.
    ///
    /// A busy/limited/retryable syscall or any network failure is worth a retry;
    /// a missing file or a hash mismatch is not.
    pub fn is_transient(&self) -> bool {
        match self.kind {
            Kind::Network => true,
            Kind::Io => matches!(
                self.io_kind(),
                Some(
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::PermissionDenied
                )
            ),
            // The other instance closing is the retry the user is being asked for.
            Kind::Busy => true,
            Kind::Integrity | Kind::Missing | Kind::Invalid | Kind::Process | Kind::Internal => {
                false
            }
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Self::io("I/O 错误", source)
    }
}

/// Shorthand for results from this crate's fallible functions.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_display_keeps_the_message_and_appends_the_os_error() {
        let error = Error::io(
            "无法创建 /tmp/x",
            io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
        );
        assert_eq!(error.to_string(), "无法创建 /tmp/x: denied");
        assert_eq!(error.kind(), Kind::Io);
    }

    #[test]
    fn a_transient_failure_tells_the_user_to_try_again() {
        assert!(Error::network("超时").user_message().contains("可以重试"));
        assert!(!Error::missing("没有版本")
            .user_message()
            .contains("可以重试"));
    }

    #[test]
    fn transient_failures_are_the_ones_worth_retrying() {
        assert!(Error::network("超时").is_transient());
        for kind in [
            io::ErrorKind::TimedOut,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::WouldBlock,
            io::ErrorKind::Interrupted,
        ] {
            assert!(
                Error::io("读", io::Error::from(kind)).is_transient(),
                "{kind:?}"
            );
        }
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::InvalidData,
            io::ErrorKind::AlreadyExists,
        ] {
            assert!(
                !Error::io("读", io::Error::from(kind)).is_transient(),
                "{kind:?}"
            );
        }
        assert!(!Error::integrity("哈希不符").is_transient());
        assert!(!Error::missing("没有版本").is_transient());
    }

    #[test]
    fn the_source_is_reachable_through_the_standard_trait() {
        let error = Error::io("写", io::Error::from(io::ErrorKind::NotFound));
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(error.io_kind(), Some(io::ErrorKind::NotFound));
    }
}
