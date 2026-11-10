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
use std::time::Duration;

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
}

/// How often the worker reports its position while a track plays.
const TICK: Duration = Duration::from_millis(120);

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
                    match DeviceSinkBuilder::open_default_sink() {
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
