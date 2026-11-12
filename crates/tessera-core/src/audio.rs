//! The audio worker: rodio behind two channels, so the state machine never
//! touches a device.
//!
//! Only Ogg Vorbis is compiled in. The game ships nothing else, and
//! `rodio = { default-features = false, features = ["vorbis"] }` keeps every
//! other decoder (flac, mp3, mp4/aac, wav) out of the binary — those crates are
//! not even compiled.
//!
//! No FFmpeg anywhere: rodio decodes through symphonia, which is pure Rust, and
//! the output backend is cpal: WASAPI on Windows, CoreAudio on macOS, ALSA on
//! Linux. Nothing has to be installed at runtime on any of them.
//!
//! The device is opened on the first `Play`, not at start-up, so the launcher
//! does not sit on an audio device (or fail to) before anyone presses play.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

/// What the state machine asks the worker to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Decode `path` from `start` and play it. `byte_len` is the object's size
    /// from the asset index: Vorbis carries no timing information, so rodio needs
    /// it to know the length and to seek.
    Play {
        path: PathBuf,
        byte_len: u64,
        start: Duration,
    },
    Pause,
    Resume,
    Stop,
    Seek(Duration),
    Volume(f32),
}

/// What the worker reports back.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Where the device actually is in the current track.
    Progress(Duration),
    /// The current track ran to its end.
    Finished,
    /// The device or the file could not be used; the transport should stop.
    Failed(String),
    /// The stream reported an error that does not stop playback. Repeated
    /// identical reports are collapsed by the worker.
    DeviceWarning(String),
}

/// How often the worker reports its position while a track plays.
const TICK: Duration = Duration::from_millis(120);

/// How long an identical stream warning is suppressed after it was reported.
///
/// ALSA's `pulse` plugin returns `-EIO` from `snd_pcm_avail_delay()` while the
/// server has no timing information yet — a documented plugin bug
/// (alsa-project/alsa-plugins#65) that fires right after the PCM is configured
/// and resolves by itself. cpal keeps the stream running through it, so it is a
/// warning, not a failure; without this filter rodio prints it on every poll.
const WARNING_INTERVAL: Duration = Duration::from_secs(30);

/// Collapses repeats of the same stream error.
#[derive(Default)]
struct WarningFilter {
    last: Option<(String, Instant)>,
}

impl WarningFilter {
    /// Whether this message is new enough to report. `now` is a parameter so the
    /// interval can be tested without sleeping.
    fn accept(&mut self, message: &str, now: Instant) -> bool {
        if let Some((previous, at)) = self.last.as_ref() {
            if previous == message && now.duration_since(*at) < WARNING_INTERVAL {
                return false;
            }
        }
        self.last = Some((message.to_string(), now));
        true
    }
}

/// Open the default sink with our own stream-error callback.
///
/// rodio's default callback writes every error to stderr, which is how a
/// transient ALSA hiccup ended up looking like a crash in the terminal. This one
/// routes the message to the UI log instead, at most once per
/// [`WARNING_INTERVAL`] per distinct message.
fn open_device(events: &Sender<Event>) -> Result<MixerDeviceSink, String> {
    let filter: Arc<Mutex<WarningFilter>> = Arc::new(Mutex::new(WarningFilter::default()));
    let reporter = events.clone();
    let callback = move |error: rodio::CpalError| {
        let message = error.to_string();
        let accepted = filter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .accept(&message, Instant::now());
        if accepted {
            let _ = reporter.send(Event::DeviceWarning(format!(
                "音频流警告（播放继续）：{message}"
            )));
        }
    };
    let first = DeviceSinkBuilder::from_default_device()
        .map_err(|error| error.to_string())?
        .with_error_callback(callback.clone())
        .open_sink_or_fallback();
    if let Ok(sink) = first {
        return Ok(sink);
    }
    let first_error = first.expect_err("checked above");
    // Same fallback rodio's own `open_default_sink` performs: a box whose default
    // output is unusable often has a second device that works.
    if let Ok(devices) = rodio::cpal::default_host().output_devices() {
        for device in devices {
            let usable = device
                .description()
                .map(|description| description.driver().is_some_and(|driver| driver != "null"))
                .unwrap_or(false);
            if !usable {
                continue;
            }
            if let Ok(builder) = DeviceSinkBuilder::from_device(device) {
                if let Ok(sink) = builder
                    .with_error_callback(callback.clone())
                    .open_sink_or_fallback()
                {
                    return Ok(sink);
                }
            }
        }
    }
    Err(first_error.to_string())
}

/// Start the worker thread.
pub fn spawn() -> (Sender<Command>, Receiver<Event>) {
    let (commands_tx, commands_rx) = mpsc::channel::<Command>();
    let (events_tx, events_rx) = mpsc::channel::<Event>();
    std::thread::Builder::new()
        .name("tessera-audio".to_string())
        .spawn(move || run(commands_rx, events_tx))
        .expect("failed to start the audio thread");
    (commands_tx, events_rx)
}

fn run(commands: Receiver<Command>, events: Sender<Event>) {
    // Opened lazily; kept alive for the whole thread so the device stream is not
    // dropped between tracks.
    let mut device: Option<MixerDeviceSink> = None;
    let mut player: Option<Player> = None;
    let mut volume: f32 = 0.7;
    let mut playing = false;

    loop {
        match commands.recv_timeout(TICK) {
            Ok(Command::Play {
                path,
                byte_len,
                start,
            }) => {
                if device.is_none() {
                    match open_device(&events) {
                        Ok(opened) => {
                            let created = Player::connect_new(opened.mixer());
                            created.set_volume(volume);
                            player = Some(created);
                            device = Some(opened);
                        }
                        Err(error) => {
                            playing = false;
                            let _ = events.send(Event::Failed(format!("音频设备打不开：{error}")));
                            continue;
                        }
                    }
                }
                let Some(current) = &player else { continue };
                let file = match std::fs::File::open(&path) {
                    Ok(file) => file,
                    Err(error) => {
                        playing = false;
                        let _ = events
                            .send(Event::Failed(format!("打不开 {}：{error}", path.display())));
                        continue;
                    }
                };
                let decoder = match Decoder::builder()
                    .with_data(std::io::BufReader::new(file))
                    .with_byte_len(byte_len)
                    .build()
                {
                    Ok(decoder) => decoder,
                    Err(error) => {
                        playing = false;
                        let _ = events.send(Event::Failed(format!(
                            "解码 {} 失败：{error}",
                            path.display()
                        )));
                        continue;
                    }
                };
                current.clear();
                current.append(decoder);
                if !start.is_zero() && current.try_seek(start).is_err() {
                    // A seek past the end (or into an unseekable stream) is not
                    // worth failing the track over; it just starts at the top.
                    let _ = events.send(Event::Progress(Duration::ZERO));
                }
                current.play();
                playing = true;
                let _ = events.send(Event::Progress(current.get_pos()));
            }
            Ok(Command::Pause) => {
                if let Some(current) = &player {
                    current.pause();
                }
                playing = false;
            }
            Ok(Command::Resume) => {
                if let Some(current) = &player {
                    current.play();
                    playing = true;
                }
            }
            Ok(Command::Stop) => {
                if let Some(current) = &player {
                    current.clear();
                }
                playing = false;
            }
            Ok(Command::Seek(position)) => {
                if let Some(current) = &player {
                    match current.try_seek(position) {
                        Ok(()) => {
                            let _ = events.send(Event::Progress(current.get_pos()));
                        }
                        Err(error) => {
                            let _ = events.send(Event::Failed(format!("跳转失败：{error}")));
                        }
                    }
                }
            }
            Ok(Command::Volume(value)) => {
                volume = value.clamp(0.0, 1.0);
                if let Some(current) = &player {
                    current.set_volume(volume);
                }
            }
            // Nothing asked for this tick: report progress, or say the track ended.
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if let Some(current) = &player {
            if playing {
                if current.empty() {
                    playing = false;
                    let _ = events.send(Event::Finished);
                } else if events.send(Event::Progress(current.get_pos())).is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_stream_error_is_reported_once_per_interval() {
        let mut filter = WarningFilter::default();
        let now = Instant::now();
        assert!(filter.accept("avail_delay: EIO", now));
        assert!(!filter.accept("avail_delay: EIO", now + WARNING_INTERVAL / 2));
        assert!(!filter.accept(
            "avail_delay: EIO",
            now + WARNING_INTERVAL - Duration::from_millis(1)
        ));
        assert!(filter.accept("avail_delay: EIO", now + WARNING_INTERVAL));
    }

    #[test]
    fn a_different_error_is_reported_immediately() {
        let mut filter = WarningFilter::default();
        let now = Instant::now();
        assert!(filter.accept("first", now));
        assert!(filter.accept("second", now));
    }
}
