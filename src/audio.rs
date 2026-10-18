//! Microphone capture and playback through PulseAudio.
//!
//! WSLg exposes a PulseAudio server, and native Linux desktops expose the same
//! API through PipeWire, so one backend covers both. Audio stays in memory.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use libpulse_binding::def::BufferAttr;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;

/// Whisper-style input: 16 kHz, mono, signed 16-bit.
pub const SAMPLE_RATE: u32 = 16_000;
/// Samples per read. 20 ms keeps the level meter and key handling responsive.
const CHUNK_SAMPLES: usize = (SAMPLE_RATE as usize) / 50;
const APP_NAME: &str = "falai";
/// Longest wait for audio still in flight after the key is released. WSLg's
/// RDP microphone delivers audio in bursts of up to ~200 ms.
const MAX_TAIL_WAIT: Duration = Duration::from_millis(600);

fn spec() -> Spec {
    Spec {
        format: Format::S16le,
        channels: 1,
        rate: SAMPLE_RATE,
    }
}

#[derive(Default)]
struct Shared {
    recording: bool,
    started: Option<Instant>,
    buffer: Vec<i16>,
    /// Peak of the most recent chunk, 0.0..=1.0.
    level: f32,
}

/// Reads the microphone continuously on a background thread and keeps the
/// samples only while recording is on.
pub struct Capture {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Capture {
    pub fn start() -> Result<Self> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

        let handle = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let attr = BufferAttr {
                    maxlength: u32::MAX,
                    tlength: u32::MAX,
                    prebuf: u32::MAX,
                    minreq: u32::MAX,
                    // Ask the server to deliver data in small fragments.
                    fragsize: (CHUNK_SAMPLES * 2) as u32,
                };
                let stream = match Simple::new(
                    None,
                    APP_NAME,
                    Direction::Record,
                    None,
                    "microfone",
                    &spec(),
                    None,
                    Some(&attr),
                ) {
                    Ok(s) => {
                        let _ = ready_tx.send(Ok(()));
                        s
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(anyhow!("{e}")));
                        return;
                    }
                };

                let mut bytes = vec![0u8; CHUNK_SAMPLES * 2];
                while !stop.load(Ordering::Relaxed) {
                    if stream.read(&mut bytes).is_err() {
                        break;
                    }
                    let samples: Vec<i16> = bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| i16::from_le_bytes(*b))
                        .collect();
                    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);

                    let mut st = shared.lock().unwrap();
                    st.level = peak as f32 / i16::MAX as f32;
                    if st.recording {
                        st.buffer.extend_from_slice(&samples);
                    }
                }
            })
        };

        ready_rx
            .recv()
            .context("a thread de captura terminou antes de abrir o microfone")?
            .context("não foi possível abrir o microfone pelo PulseAudio")?;

        Ok(Self {
            shared,
            stop,
            handle: Some(handle),
        })
    }

    pub fn begin(&self) {
        let mut st = self.shared.lock().unwrap();
        st.buffer.clear();
        st.started = Some(Instant::now());
        st.recording = true;
    }

    /// Stops recording and hands back what was captured.
    ///
    /// Audio arrives with some delay, so at release time the last moments of
    /// speech may still be in flight. Keep reading until the buffer covers the
    /// whole time the key was held, up to `MAX_TAIL_WAIT`.
    pub fn end(&self) -> Recording {
        let released = Instant::now();
        loop {
            {
                let mut st = self.shared.lock().unwrap();
                let held = st.started.map_or(Duration::ZERO, |s| released - s);
                let wanted = (held.as_secs_f64() * SAMPLE_RATE as f64) as usize;
                if st.buffer.len() >= wanted || released.elapsed() >= MAX_TAIL_WAIT {
                    st.recording = false;
                    st.started = None;
                    return Recording {
                        samples: std::mem::take(&mut st.buffer),
                        tail_wait: released.elapsed(),
                    };
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn level(&self) -> f32 {
        self.shared.lock().unwrap().level
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

pub struct Recording {
    pub samples: Vec<i16>,
    /// How long `end` waited for in-flight audio.
    pub tail_wait: Duration,
}

/// Plays samples and blocks until they finish.
pub fn play(samples: &[i16]) -> Result<()> {
    let stream = Simple::new(
        None,
        APP_NAME,
        Direction::Playback,
        None,
        "reprodução",
        &spec(),
        None,
        None,
    )
    .map_err(|e| anyhow!("{e}"))
    .context("não foi possível abrir a saída de áudio pelo PulseAudio")?;

    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    stream.write(&bytes).map_err(|e| anyhow!("{e}"))?;
    stream.drain().map_err(|e| anyhow!("{e}"))?;
    Ok(())
}

pub struct Stats {
    pub seconds: f32,
    pub peak_dbfs: f32,
    pub rms_dbfs: f32,
}

pub fn stats(samples: &[i16]) -> Stats {
    let seconds = samples.len() as f32 / SAMPLE_RATE as f32;
    if samples.is_empty() {
        return Stats {
            seconds,
            peak_dbfs: f32::NEG_INFINITY,
            rms_dbfs: f32::NEG_INFINITY,
        };
    }
    let full = i16::MAX as f32;
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap() as f32 / full;
    let mean_sq = samples
        .iter()
        .map(|&s| {
            let x = s as f32 / full;
            x * x
        })
        .sum::<f32>()
        / samples.len() as f32;
    Stats {
        seconds,
        peak_dbfs: 20.0 * peak.log10(),
        rms_dbfs: 10.0 * mean_sq.log10(),
    }
}
