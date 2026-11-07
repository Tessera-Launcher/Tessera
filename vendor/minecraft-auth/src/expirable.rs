use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use crate::error::Result;

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the unix epoch")
        .as_millis() as i64
}

/// A value with a known expiry.
pub trait Expirable {
    fn expire_time_ms(&self) -> i64;

    fn is_expired(&self) -> bool {
        self.expire_time_ms() <= now_ms()
    }
}

/// A lazily refreshed, expiry-aware cache for one pipeline stage.
///
/// `get_up_to_date` holds the lock for the whole refresh, so concurrent
/// callers coalesce onto a single in-flight request instead of firing one
/// each. Because the lock is held across the refresh, the refresh closure
/// must never call back into the *same* holder — `tokio::sync::Mutex` is
/// not reentrant, so that would deadlock forever rather than fail. The
/// closure is handed the currently cached value for exactly this reason:
/// a refresh that needs the old value (e.g. to read a refresh token) gets
/// it as an argument instead of reaching for `cached()`.
pub struct Holder<T> {
    state: Mutex<Option<T>>,
}

impl<T: Expirable + Clone> Holder<T> {
    pub fn empty() -> Self {
        Self {
            state: Mutex::new(None),
        }
    }

    pub fn with_value(value: T) -> Self {
        Self {
            state: Mutex::new(Some(value)),
        }
    }

    pub async fn cached(&self) -> Option<T> {
        self.state.lock().await.clone()
    }

    /// Returns the cached value if it is still valid, otherwise refreshes it.
    ///
    /// `refresh` receives the currently cached value (if any). It must not
    /// touch this holder again — see the type-level note on reentrancy.
    pub async fn get_up_to_date<F, Fut>(&self, refresh: F) -> Result<T>
    where
        F: FnOnce(Option<T>) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut guard = self.state.lock().await;
        let is_stale = match guard.as_ref() {
            Some(value) => value.is_expired(),
            None => true,
        };
        if is_stale {
            let fresh = refresh(guard.clone()).await?;
            *guard = Some(fresh.clone());
            Ok(fresh)
        } else {
            Ok(guard.clone().expect("checked above"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Token {
        expire_time_ms: i64,
        value: &'static str,
    }

    impl Expirable for Token {
        fn expire_time_ms(&self) -> i64 {
            self.expire_time_ms
        }
    }

    fn expired(value: &'static str) -> Token {
        Token {
            expire_time_ms: 0,
            value,
        }
    }

    fn valid(value: &'static str) -> Token {
        Token {
            expire_time_ms: now_ms() + 60_000,
            value,
        }
    }

    /// The refresh closure is handed the expired value, so a refresh that
    /// needs it (reading a refresh token, say) never has to call `cached()`
    /// — which would deadlock against the lock `get_up_to_date` still holds.
    #[tokio::test]
    async fn refresh_receives_the_expired_value() {
        let holder = Holder::with_value(expired("old"));

        let fresh = holder
            .get_up_to_date(|cached| async move {
                assert_eq!(cached, Some(expired("old")));
                Ok(valid("new"))
            })
            .await
            .unwrap();

        assert_eq!(fresh, valid("new"));
        assert_eq!(holder.cached().await, Some(valid("new")));
    }

    #[tokio::test]
    async fn refresh_receives_none_when_nothing_is_cached() {
        let holder: Holder<Token> = Holder::empty();

        holder
            .get_up_to_date(|cached| async move {
                assert_eq!(cached, None);
                Ok(valid("new"))
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn valid_value_is_returned_without_refreshing() {
        let holder = Holder::with_value(valid("current"));

        let value = holder
            .get_up_to_date(|_| async { panic!("must not refresh a valid value") })
            .await
            .unwrap();

        assert_eq!(value, valid("current"));
    }
}
