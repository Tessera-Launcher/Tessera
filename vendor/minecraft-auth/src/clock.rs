use std::time::SystemTime;

use tokio::sync::{Mutex, OnceCell};
use tokio::time::Instant;

/// How long a sample is trusted. Server and local clocks drift, and the local
/// one can be corrected while the process runs, so the offset is re-sampled
/// periodically instead of being cached for the process lifetime.
const SAMPLE_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

struct Sample {
    offset_seconds: i64,
    sampled_at: Instant,
}

impl Sample {
    fn is_stale(&self) -> bool {
        self.sampled_at.elapsed() >= SAMPLE_TTL
    }
}

static SAMPLE: OnceCell<Mutex<Option<Sample>>> = OnceCell::const_new();

/// Seconds to add to the local clock to match Microsoft's servers, sampled
/// from `login.live.com`'s `Date` header. Xbox Live rejects signed requests
/// whose timestamp drifts too far, so a wrong local clock would otherwise
/// break every signed request.
///
/// A failed sample is never cached: the caller gets the last good value (or 0
/// when there has never been one) and the next call tries again, so a
/// temporary network failure cannot poison every later signed request.
pub async fn offset_seconds(client: &reqwest::Client) -> i64 {
    let sample = SAMPLE.get_or_init(|| async { Mutex::new(None) }).await;
    let mut sample = sample.lock().await;

    if !sample.as_ref().is_some_and(|sample| !sample.is_stale()) {
        if let Some(offset_seconds) = fetch_offset(client).await {
            *sample = Some(Sample {
                offset_seconds,
                sampled_at: Instant::now(),
            });
        }
    }

    sample.as_ref().map_or(0, |sample| sample.offset_seconds)
}

async fn fetch_offset(client: &reqwest::Client) -> Option<i64> {
    let response = client
        .get("https://login.live.com/")
        .timeout(crate::REQUEST_TIMEOUT)
        .send()
        .await
        .ok()?;
    let date_header = response
        .headers()
        .get(reqwest::header::DATE)?
        .to_str()
        .ok()?;
    let server_time = httpdate::parse_http_date(date_header).ok()?;
    let client_time = SystemTime::now();
    Some(match server_time.duration_since(client_time) {
        Ok(ahead) => ahead.as_secs() as i64,
        Err(behind) => -(behind.duration().as_secs() as i64),
    })
}
