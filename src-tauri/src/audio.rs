//! F3 audio audition: one playback at a time in the main window. fxfm-core decodes, this
//! module resamples to the output device and plays through cpal. The engine talks to the
//! device through `Sink`, so tests run it against a fake one.

use crate::commands::needs_copy;
use crate::remote::watch;
use crate::state::{lock, AppState};
use fxfm_core::audio::{self as decode, clock, AudioError, AudioInfo, AudioSource};
use fxfm_core::vfs;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

pub const STATE_EVENT: &str = "audio://state";
/// `audio_play` error when a later F3 or a stop replaced the request (the UI ignores it).
pub const SUPERSEDED: &str = "superseded";
pub const NO_DEVICE: &str = "Cannot play: no audio output device";
/// Position updates while playing.
const TICK: Duration = Duration::from_millis(250);
/// How long the engine waits when the device buffer is full.
const IDLE: Duration = Duration::from_millis(5);

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlayState {
    Loading,
    Playing,
    Paused,
    Ended,
    Stopped,
    Error,
}

/// Payload of `audio://state`.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct StateEvent {
    /// The request (F3 press or stop) the event belongs to; ids only grow, so the UI drops
    /// events older than its newest request.
    pub id: u64,
    /// The location as F3 got it (inside archives and on servers too).
    pub path: String,
    pub state: PlayState,
    pub position: f64,
    pub duration: Option<f64>,
    /// Error text; in `playing` events a note for the strip's codec line.
    pub message: Option<String>,
    /// Codec, rate and channels (in `playing` events).
    pub info: Option<AudioInfo>,
}

impl StateEvent {
    fn new(id: u64, path: &str, state: PlayState) -> StateEvent {
        StateEvent {
            id,
            path: path.to_string(),
            state,
            position: 0.0,
            duration: None,
            message: None,
            info: None,
        }
    }

    fn error(id: u64, path: &str, message: String) -> StateEvent {
        StateEvent {
            message: Some(message),
            ..StateEvent::new(id, path, PlayState::Error)
        }
    }
}

/// What `audio_play` returns: the request id and the stream's `AudioInfo` fields.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Started {
    pub id: u64,
    #[serde(flatten)]
    pub info: AudioInfo,
}

pub type Emit = Arc<dyn Fn(&StateEvent) + Send + Sync>;
/// Removes the temporary copy of an archive or server file.
pub type Cleanup = Box<dyn FnOnce() + Send>;

/// Runs its cleanup when dropped, whichever way a request ends (also on a panic).
struct TempGuard(Option<Cleanup>);

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(c) = self.0.take() {
            c();
        }
    }
}

// ---------------------------------------------------------------- output

/// An open output device. It lives on the audio thread, open across playbacks.
pub trait Sink {
    fn rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// Queues as many interleaved samples as fit (whole frames); returns how many it took.
    fn push(&mut self, samples: &[f32]) -> usize;
    /// Frames queued and not played yet.
    fn queued(&self) -> usize;
    /// Drops everything queued (seek).
    fn clear(&mut self);
    /// Set when the device failed (unplugged...).
    fn failure(&self) -> Option<String> {
        None
    }
}

/// An output the factory opened.
pub struct Opened {
    pub sink: Box<dyn Sink>,
    /// The named device is not connected: this is the system default instead.
    pub fallback: bool,
    /// Name of the device opened (None when unknown).
    pub device: Option<String>,
}

pub trait SinkFactory: Send + Sync {
    /// Opens the output device `device` (an id, or the first of that name), or the system default
    /// when it is None or not connected; `Err` carries the message for the strip.
    fn open(&self, device: Option<&str>) -> Result<Opened, String>;
    /// Name of the device `open(device)` would use now (None when unknown); checked while
    /// playing, so a new system default or a reconnected chosen device is taken at once.
    fn current(&self, _device: Option<&str>) -> Option<String> {
        None
    }
}

/// The output devices through cpal.
pub struct CpalFactory;

/// An output device for Options: its name and cpal id (`coreaudio:<UID>`, stable across
/// reconnects). Devices that share a name are told apart by the id.
#[derive(serde::Serialize, Clone, Debug, PartialEq, Eq)]
pub struct OutputDevice {
    pub name: String,
    pub id: Option<String>,
}

/// The output devices in the order the system lists them (empty on error). Does not open a
/// stream.
pub fn outputs() -> Vec<OutputDevice> {
    use cpal::traits::{DeviceTrait, HostTrait};
    match cpal::default_host().output_devices() {
        Ok(devs) => devs
            .filter_map(|d| {
                let name = d.description().ok()?.name().to_string();
                Some(OutputDevice {
                    name,
                    id: d.id().ok().map(|i| i.to_string()),
                })
            })
            .collect(),
        Err(e) => {
            eprintln!("fxfm: audio outputs: {e}");
            Vec::new()
        }
    }
}

/// Which of `devices` the saved choice `sel` means: the device with that id, else the first
/// one with that name (older settings and devices with a unique name store the name).
pub fn pick_output(devices: &[OutputDevice], sel: &str) -> Option<usize> {
    devices
        .iter()
        .position(|d| d.id.as_deref() == Some(sel))
        .or_else(|| devices.iter().position(|d| d.name == sel))
}

/// The output device the saved choice `sel` means (see [`pick_output`]).
fn find_output(host: &cpal::Host, sel: &str) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let devs: Vec<cpal::Device> = host.output_devices().ok()?.collect();
    let info: Vec<OutputDevice> = devs
        .iter()
        .map(|d| OutputDevice {
            name: d
                .description()
                .map(|x| x.name().to_string())
                .unwrap_or_default(),
            id: d.id().ok().map(|i| i.to_string()),
        })
        .collect();
    pick_output(&info, sel).map(|i| devs[i].clone())
}

struct Queue {
    samples: std::collections::VecDeque<f32>,
    failure: Option<String>,
}

struct CpalSink {
    _stream: cpal::Stream,
    q: Arc<Mutex<Queue>>,
    rate: u32,
    channels: u16,
    /// Samples the queue holds at most (~200 ms).
    cap: usize,
}

impl SinkFactory for CpalFactory {
    fn open(&self, device: Option<&str>) -> Result<Opened, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use cpal::SampleFormat as F;
        let host = cpal::default_host();
        let named = device.and_then(|n| find_output(&host, n));
        let fallback = device.is_some() && named.is_none();
        let dev = match named {
            Some(d) => d,
            None => host.default_output_device().ok_or(NO_DEVICE)?,
        };
        let sc = dev
            .default_output_config()
            .map_err(|e| format!("{NO_DEVICE} ({e})"))?;
        let config = sc.config();
        let (rate, channels) = (config.sample_rate, config.channels);
        let cap = (rate as usize / 5).max(1024) * usize::from(channels);
        let q = Arc::new(Mutex::new(Queue {
            samples: std::collections::VecDeque::with_capacity(cap),
            failure: None,
        }));
        let stream = match sc.sample_format() {
            F::F32 => build::<f32>(&dev, config, &q),
            F::I16 => build::<i16>(&dev, config, &q),
            F::I32 => build::<i32>(&dev, config, &q),
            F::U16 => build::<u16>(&dev, config, &q),
            f => {
                return Err(format!(
                    "Cannot play: output sample format {f} not supported"
                ))
            }
        }
        .map_err(|e| format!("Cannot play: {e}"))?;
        stream.play().map_err(|e| format!("Cannot play: {e}"))?;
        Ok(Opened {
            sink: Box::new(CpalSink {
                _stream: stream,
                q,
                rate,
                channels,
                cap,
            }),
            fallback,
            device: device_name(&dev),
        })
    }

    fn current(&self, device: Option<&str>) -> Option<String> {
        use cpal::traits::HostTrait;
        let host = cpal::default_host();
        device
            .and_then(|n| find_output(&host, n))
            .or_else(|| host.default_output_device())
            .and_then(|d| device_name(&d))
    }
}

fn device_name(d: &cpal::Device) -> Option<String> {
    use cpal::traits::DeviceTrait;
    d.description().ok().map(|x| x.name().to_string())
}

fn build<T>(
    dev: &cpal::Device,
    config: cpal::StreamConfig,
    q: &Arc<Mutex<Queue>>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    let (data_q, err_q) = (q.clone(), q.clone());
    dev.build_output_stream::<T, _, _>(
        config,
        move |out: &mut [T], _| {
            let mut q = lock(&data_q);
            for s in out.iter_mut() {
                *s = <T as cpal::Sample>::from_sample(q.samples.pop_front().unwrap_or(0.0));
            }
        },
        move |e| lock(&err_q).failure = Some(e.to_string()),
        None,
    )
}

impl Sink for CpalSink {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn push(&mut self, samples: &[f32]) -> usize {
        let mut q = lock(&self.q);
        let ch = usize::from(self.channels);
        let room = self.cap.saturating_sub(q.samples.len());
        let n = samples.len().min(room) / ch * ch;
        q.samples.extend(&samples[..n]);
        n
    }

    fn queued(&self) -> usize {
        lock(&self.q).samples.len() / usize::from(self.channels)
    }

    fn clear(&mut self) {
        lock(&self.q).samples.clear();
    }

    fn failure(&self) -> Option<String> {
        lock(&self.q).failure.clone()
    }
}

// ---------------------------------------------------------------- resampling

/// Phases of the interpolation table.
const PHASES: usize = 256;
/// Filter half-width in input samples (when not downsampling).
const HALF_TAPS: f64 = 16.0;

/// Streaming windowed-sinc resampler (Blackman window, cutoff 0.46 of the lower rate),
/// interleaved in and out.
pub struct Resampler {
    ch: usize,
    passthrough: bool,
    /// Input samples per output sample.
    step: f64,
    half: usize,
    /// `(PHASES + 1) * 2 * half` filter coefficients.
    table: Vec<f32>,
    /// Per channel: history, then the samples not consumed yet.
    buf: Vec<Vec<f32>>,
    /// Position of the next output sample in `buf`.
    pos: f64,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32, channels: usize) -> Resampler {
        let passthrough = in_rate == out_rate;
        // At most 1:64 downsampling is filtered properly (caps the table at 1024 taps a phase;
        // `fxfm_core::audio::open` already refuses rates above 768 kHz).
        let scale = (f64::from(out_rate) / f64::from(in_rate.max(1))).clamp(1.0 / 64.0, 1.0);
        let half = (HALF_TAPS / scale).ceil() as usize;
        let taps = 2 * half;
        let fc = 0.46 * scale; // cycles per input sample
        let mut table = vec![0f32; (PHASES + 1) * taps];
        if !passthrough {
            for p in 0..=PHASES {
                let row = &mut table[p * taps..(p + 1) * taps];
                let mut sum = 0.0;
                let mut h = vec![0f64; taps];
                for (k, hk) in h.iter_mut().enumerate() {
                    // Distance from the output point to input sample k of the window.
                    let d = p as f64 / PHASES as f64 + (half as f64 - 1.0) - k as f64;
                    let u = d / half as f64;
                    let w = if u.abs() >= 1.0 {
                        0.0
                    } else {
                        let a = std::f64::consts::PI * u;
                        0.42 + 0.5 * a.cos() + 0.08 * (2.0 * a).cos()
                    };
                    let x = 2.0 * fc * d;
                    let sinc = if x == 0.0 {
                        1.0
                    } else {
                        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                    };
                    *hk = 2.0 * fc * sinc * w;
                    sum += *hk;
                }
                for (r, hk) in row.iter_mut().zip(h) {
                    *r = (hk / sum) as f32;
                }
            }
        }
        let mut r = Resampler {
            ch: channels.max(1),
            passthrough,
            step: f64::from(in_rate) / f64::from(out_rate),
            half,
            table,
            buf: Vec::new(),
            pos: 0.0,
        };
        r.reset();
        r
    }

    /// Forgets the stream (seek).
    pub fn reset(&mut self) {
        self.buf = vec![vec![0.0; self.half]; self.ch];
        self.pos = self.half as f64;
    }

    /// Appends the output for `input` to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough {
            out.extend_from_slice(input);
            return;
        }
        for frame in input.chunks_exact(self.ch) {
            for (b, &s) in self.buf.iter_mut().zip(frame) {
                b.push(s);
            }
        }
        self.run(out);
    }

    /// Appends the tail held back by the filter (end of stream).
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if self.passthrough {
            return;
        }
        for b in self.buf.iter_mut() {
            b.extend(std::iter::repeat_n(0.0, self.half));
        }
        self.run(out);
        self.reset();
    }

    fn run(&mut self, out: &mut Vec<f32>) {
        let taps = 2 * self.half;
        let len = self.buf[0].len();
        loop {
            let i0 = self.pos.floor() as usize;
            if i0 + self.half >= len {
                break;
            }
            let ph = (self.pos - i0 as f64) * PHASES as f64;
            let p = (ph as usize).min(PHASES - 1);
            let q = (ph - p as f64) as f32;
            let (r0, r1) = (
                &self.table[p * taps..(p + 1) * taps],
                &self.table[(p + 1) * taps..(p + 2) * taps],
            );
            let first = i0 + 1 - self.half;
            for b in &self.buf {
                let x = &b[first..first + taps];
                let mut acc = 0f32;
                for k in 0..taps {
                    acc += x[k] * (r0[k] + q * (r1[k] - r0[k]));
                }
                out.push(acc);
            }
            self.pos += self.step;
        }
        // Keep the history the next outputs need.
        let drop = (self.pos.floor() as usize + 1).saturating_sub(self.half);
        if drop > 0 {
            for b in self.buf.iter_mut() {
                b.drain(..drop.min(b.len()));
            }
            self.pos -= drop as f64;
        }
    }
}

/// Interleaved `from`-channel audio as `to` channels: mono goes to every channel, many to mono
/// is averaged, otherwise channels are taken in order (missing ones silent).
pub fn map_channels(input: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
    if from == to {
        out.extend_from_slice(input);
        return;
    }
    for f in input.chunks_exact(from) {
        if from == 1 {
            out.extend(std::iter::repeat_n(f[0], to));
        } else if to == 1 {
            out.push(f.iter().sum::<f32>() / from as f32);
        } else {
            out.extend((0..to).map(|c| f.get(c).copied().unwrap_or(0.0)));
        }
    }
}

// ---------------------------------------------------------------- engine

/// Last path component of a location (local, archive or server).
fn file_name(location: &str) -> &str {
    location
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(location)
}

/// The strip's text for a file that cannot start.
fn open_error(location: &str, e: &AudioError) -> String {
    format!("Cannot play {}: {e}", file_name(location))
}

/// The strip's text after a panic while decoding.
fn panic_message(location: &str) -> String {
    format!("Cannot play {}: file is damaged", file_name(location))
}

/// The output closes after this long without playback.
const IDLE_CLOSE: Duration = Duration::from_secs(60);

/// The strip's note when the chosen device is missing.
fn fallback_notice(name: &str) -> String {
    format!("Output: {name} is not connected, using the system default")
}

/// The output device the user chose, shared with the audio thread.
#[derive(Default)]
struct Output {
    /// Device name; None = the system default.
    wanted: Mutex<Option<String>>,
    /// Missing devices already reported (once per app run).
    warned: Mutex<std::collections::HashSet<String>>,
    /// Attenuation as f32 bits: 0.0 = full volume, 1.0 = silent (so the default is full).
    quiet: std::sync::atomic::AtomicU32,
    /// Name of the device the audio thread has open (as the factory reported it).
    opened: Mutex<Option<String>>,
}

impl Output {
    /// The playback volume, 0.0 (silent) to 1.0 (full).
    fn volume(&self) -> f32 {
        1.0 - f32::from_bits(self.quiet.load(Ordering::Relaxed))
    }
}

/// Sends events of the active request only, so a replaced playback stays silent.
struct Gate {
    active: Mutex<u64>,
    emit: Emit,
}

impl Gate {
    fn active(&self) -> u64 {
        *lock(&self.active)
    }

    fn emit(&self, ev: &StateEvent) {
        let active = lock(&self.active);
        if *active == ev.id {
            (self.emit)(ev);
        }
    }
}

/// A file to play, opened by the caller.
struct Request {
    id: u64,
    path: String,
    src: Box<dyn AudioSource>,
    _temp: TempGuard,
}

enum Cmd {
    Play(Request, mpsc::SyncSender<Result<(), String>>),
    Stop,
    Seek(f64),
    Pause(bool),
    Shutdown(mpsc::SyncSender<()>),
}

/// One playback at a time on one audio thread. The thread keeps the output open across
/// playbacks (rapid F3 swaps only the source) and closes it after `IDLE_CLOSE`. Nothing on the
/// caller's side waits for the thread except `play` (until the sound starts) and `shutdown`.
pub struct Player {
    factory: Arc<dyn SinkFactory>,
    gate: Arc<Gate>,
    output: Arc<Output>,
    /// Request ids (F3 presses and stops).
    seq: AtomicU64,
    /// The audio thread; started on the first play and again if it died.
    tx: Mutex<Option<mpsc::Sender<Cmd>>>,
    /// Location of the latest request (for the `stopped` event while loading).
    last_path: Mutex<String>,
}

impl Player {
    pub fn new(factory: Arc<dyn SinkFactory>, emit: Emit) -> Player {
        Player {
            factory,
            gate: Arc::new(Gate {
                active: Mutex::new(0),
                emit,
            }),
            output: Arc::default(),
            seq: AtomicU64::new(0),
            tx: Mutex::new(None),
            last_path: Mutex::new(String::new()),
        }
    }

    /// A new request id, active at once: everything older is silenced and refused from now on.
    fn next_id(&self) -> u64 {
        let mut active = lock(&self.gate.active);
        let id = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        *active = id;
        id
    }

    /// Sends to the audio thread if it runs (no thread: nothing plays).
    fn send(&self, cmd: Cmd) {
        if let Some(tx) = lock(&self.tx).as_ref() {
            let _ = tx.send(cmd);
        }
    }

    /// The audio thread's channel, starting the thread when needed.
    fn sender(&self) -> Result<mpsc::Sender<Cmd>, String> {
        let mut tx = lock(&self.tx);
        if let Some(t) = tx.as_ref() {
            return Ok(t.clone());
        }
        let (t, rx) = mpsc::channel();
        let (factory, gate, output) =
            (self.factory.clone(), self.gate.clone(), self.output.clone());
        std::thread::Builder::new()
            .name("fxfm-audio".into())
            .spawn(move || worker(rx, factory.as_ref(), &gate, &output))
            .map_err(|e| format!("Cannot play: {e}"))?;
        *tx = Some(t.clone());
        Ok(t)
    }

    /// Starts a request for `path`: what plays stops now, and the id is returned. `loading`
    /// shows `Loading <name>…` (a copy must be made first).
    pub fn begin(&self, path: &str, loading: bool) -> u64 {
        let id = self.next_id();
        *lock(&self.last_path) = path.to_string();
        self.send(Cmd::Stop);
        if loading {
            self.gate
                .emit(&StateEvent::new(id, path, PlayState::Loading));
        }
        id
    }

    /// Reports a failure of request `id` before playback (e.g. the copy failed).
    pub fn fail(&self, id: u64, path: &str, message: String) {
        self.gate.emit(&StateEvent::error(id, path, message));
    }

    /// Ends request `id` before playback without a message (`stopped`).
    pub fn cancel(&self, id: u64, path: &str) {
        self.gate
            .emit(&StateEvent::new(id, path, PlayState::Stopped));
    }

    /// Plays `local` for request `id`; `path` is the location shown to the user. Returns when
    /// the sound starts. `cleanup` runs when the playback ends, however it ends.
    pub fn play(
        &self,
        id: u64,
        path: &str,
        local: &Path,
        cleanup: Option<Cleanup>,
    ) -> Result<Started, String> {
        let temp = TempGuard(cleanup);
        if self.gate.active() != id {
            return Err(SUPERSEDED.into());
        }
        let opened = std::panic::catch_unwind(|| decode::open(local));
        let src = match opened {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                let msg = open_error(path, &e);
                self.fail(id, path, msg.clone());
                return Err(msg);
            }
            Err(_) => {
                let msg = panic_message(path);
                self.fail(id, path, msg.clone());
                return Err(msg);
            }
        };
        if self.gate.active() != id {
            return Err(SUPERSEDED.into());
        }
        let info = src.info();
        let req = Request {
            id,
            path: path.to_string(),
            src,
            _temp: temp,
        };
        let (reply, answer) = mpsc::sync_channel(1);
        let sent = self
            .sender()
            .and_then(|tx| tx.send(Cmd::Play(req, reply)).map_err(|_| String::new()));
        let started = match sent {
            Ok(()) => answer.recv().unwrap_or_else(|_| Err(String::new())),
            Err(e) => Err(e),
        };
        match started {
            Ok(()) => Ok(Started { id, info }),
            Err(e) if e == SUPERSEDED => Err(e),
            Err(e) => {
                let msg = if e.is_empty() {
                    // The thread is gone (it panicked outside a playback); start a new one
                    // next time.
                    *lock(&self.tx) = None;
                    "Cannot play: audio output failed".to_string()
                } else {
                    e
                };
                self.fail(id, path, msg.clone());
                Err(msg)
            }
        }
    }

    /// Stops playback (or a pending request) and reports `stopped`. Does not wait.
    pub fn stop(&self) {
        let id = self.next_id();
        self.send(Cmd::Stop);
        let path = lock(&self.last_path).clone();
        self.gate
            .emit(&StateEvent::new(id, &path, PlayState::Stopped));
    }

    /// Stops without an event and closes the output (exit); waits up to a second.
    pub fn shutdown(&self) {
        self.next_id();
        let Some(tx) = lock(&self.tx).take() else {
            return;
        };
        let (done, wait) = mpsc::sync_channel(1);
        if tx.send(Cmd::Shutdown(done)).is_ok() {
            let _ = wait.recv_timeout(Duration::from_secs(1));
        }
    }

    /// Chooses the output device by name (None or empty = the system default). The next play
    /// opens it; a playback in progress moves to it at the same position. Does not wait.
    pub fn set_output(&self, name: Option<String>) {
        *lock(&self.output.wanted) = name.filter(|n| !n.is_empty());
    }

    /// Moves the playing file to `secs`. Does not wait.
    pub fn seek(&self, secs: f64) {
        if secs.is_finite() {
            self.send(Cmd::Seek(secs));
        }
    }

    /// Pauses (`true`) or resumes (`false`) the playing file. Does not wait.
    pub fn pause(&self, paused: bool) {
        self.send(Cmd::Pause(paused));
    }

    /// Sets the volume, 0.0 to 1.0 (clamped); it applies within a fraction of a second.
    pub fn set_volume(&self, volume: f64) {
        let v = if volume.is_finite() {
            volume.clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        self.output
            .quiet
            .store((1.0 - v).to_bits(), Ordering::Relaxed);
    }
}

/// The audio thread: owns the output and the current playback.
fn worker(rx: mpsc::Receiver<Cmd>, factory: &dyn SinkFactory, gate: &Gate, output: &Output) {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
    let mut sink: Option<Box<dyn Sink>> = None;
    // The device name `sink` was opened for (None = the system default).
    let mut sink_for: Option<String> = None;
    let mut job: Option<Job> = None;
    // When the system was last asked whether the device to use changed.
    let mut dev_checked = Instant::now();
    // Opens the wanted device unless it is open and healthy; Ok carries the fallback notice
    // when the device is missing and that was not reported yet.
    let open = |sink: &mut Option<Box<dyn Sink>>,
                sink_for: &mut Option<String>|
     -> Result<Option<String>, String> {
        let wanted = lock(&output.wanted).clone();
        if *sink_for == wanted && sink.as_ref().is_some_and(|s| s.failure().is_none()) {
            return Ok(None);
        }
        *lock(&output.opened) = None;
        *sink = None;
        match catch_unwind(AssertUnwindSafe(|| factory.open(wanted.as_deref()))) {
            Ok(Ok(o)) => {
                *sink = Some(o.sink);
                *lock(&output.opened) = o.device;
                let notice = match &wanted {
                    Some(name) if o.fallback && lock(&output.warned).insert(name.clone()) => {
                        Some(fallback_notice(name))
                    }
                    _ => None,
                };
                *sink_for = wanted;
                Ok(notice)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err("Cannot play: audio output failed".into()),
        }
    };
    loop {
        let paused = job.as_ref().is_some_and(|j| j.paused);
        let cmd = if paused {
            // Paused: wait for a command without spinning; the output stays open.
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else if job.is_some() {
            match rx.try_recv() {
                Ok(c) => Some(c),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => break,
            }
        } else {
            match rx.recv_timeout(IDLE_CLOSE) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => {
                    sink = None;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };
        match cmd {
            Some(Cmd::Stop) => {
                job = None;
                if let Some(s) = sink.as_mut() {
                    s.clear();
                }
            }
            Some(Cmd::Seek(t)) => {
                if let Some(j) = job.as_mut() {
                    j.seek = Some(t);
                    if j.paused {
                        j.paused_at = t.max(0.0);
                        j.emit(gate, PlayState::Paused, j.paused_at, None);
                    }
                }
            }
            Some(Cmd::Pause(p)) => {
                if let (Some(j), Some(s)) = (job.as_mut(), sink.as_mut()) {
                    if p && !j.paused {
                        // Stop at once: what is queued is dropped and played again on resume.
                        let at = j.seek.unwrap_or_else(|| j.position(s.as_ref()));
                        s.clear();
                        j.seek = Some(at);
                        j.paused = true;
                        j.paused_at = at;
                        j.emit(gate, PlayState::Paused, at, None);
                    } else if !p && j.paused {
                        j.paused = false;
                        j.last_tick = None;
                    }
                }
            }
            Some(Cmd::Play(req, reply)) => {
                // The old playback goes first (its copy is removed now).
                job = None;
                if gate.active() != req.id {
                    drop(req); // its copy goes before the caller hears back
                    let _ = reply.send(Err(SUPERSEDED.into()));
                    continue;
                }
                let notice = match open(&mut sink, &mut sink_for) {
                    Ok(n) => n,
                    Err(e) => {
                        drop(req);
                        let _ = reply.send(Err(e));
                        continue;
                    }
                };
                // Opening the device may have taken a while.
                if gate.active() != req.id {
                    drop(req); // its copy goes before the caller hears back
                    let _ = reply.send(Err(SUPERSEDED.into()));
                    continue;
                }
                let s = sink.as_mut().expect("opened above");
                s.clear();
                let mut j = Job::new(req, s.rate(), s.channels());
                j.notice = notice;
                job = Some(j);
                let _ = reply.send(Ok(()));
            }
            Some(Cmd::Shutdown(done)) => {
                drop(job.take());
                drop(sink.take());
                let _ = done.send(());
                return;
            }
            None => {}
        }
        let (Some(j), Some(s)) = (job.as_mut(), sink.as_mut()) else {
            continue;
        };
        if j.paused {
            continue;
        }
        j.volume = output.volume();
        // Every 2 s: has the device to use changed (a new system default, the chosen device
        // connected again)? Then move there like after a device loss.
        let mut moved = false;
        if dev_checked.elapsed() >= Duration::from_secs(2) {
            dev_checked = Instant::now();
            let have = lock(&output.opened).clone();
            if let (Some(have), Some(now)) = (have, factory.current(sink_for.as_deref())) {
                moved = have != now;
            }
        }
        // A new device was chosen in Options: move there like after a device loss.
        let step = if *lock(&output.wanted) != sink_for || moved {
            Ok(Step::DeviceLost("output device changed".into()))
        } else {
            catch_unwind(AssertUnwindSafe(|| j.step(s.as_mut(), gate)))
        };
        match step {
            Ok(Step::Busy) => {}
            Ok(Step::Wait) => std::thread::sleep(IDLE),
            Ok(Step::Done) => job = None,
            Ok(Step::DeviceLost(why)) => {
                // Reopen (default device changed or vanished, or another one chosen) and go
                // on where it was.
                let at = j.position(s.as_ref());
                sink = None;
                match open(&mut sink, &mut sink_for) {
                    Ok(notice) => {
                        let s = sink.as_ref().expect("opened above");
                        j.rebind(s.rate(), s.channels(), at);
                        j.notice = notice;
                    }
                    Err(e) => {
                        j.emit(
                            gate,
                            PlayState::Error,
                            at,
                            Some(format!("Stopped: {e} ({why})")),
                        );
                        job = None;
                    }
                }
            }
            Err(_) => {
                let (id, path) = (j.id, j.path.clone());
                job = None;
                if let Some(s) = sink.as_mut() {
                    s.clear();
                }
                gate.emit(&StateEvent::error(id, &path, panic_message(&path)));
            }
        }
    }
}

enum Step {
    /// More to do at once.
    Busy,
    /// The device buffer is full or draining.
    Wait,
    Done,
    /// The output failed; the text says why.
    DeviceLost(String),
}

/// A playback on the audio thread. Dropping it removes the temporary copy.
struct Job {
    id: u64,
    path: String,
    src: Box<dyn AudioSource>,
    info: AudioInfo,
    _temp: TempGuard,
    seek: Option<f64>,
    rs: Resampler,
    dev_rate: f64,
    dev_ch: usize,
    /// Seconds where decoding (re)started, and source frames decoded since.
    base: f64,
    decoded: u64,
    resampled: Vec<f32>,
    /// Device-format samples, `sent` of them taken by the device.
    pending: Vec<f32>,
    sent: usize,
    /// Set once the source is done: None = clean end, Some(msg) = damaged/failed.
    end: Option<Option<String>>,
    last_tick: Option<Instant>,
    /// Sent as the message of `playing` events (the chosen device is missing).
    notice: Option<String>,
    /// Paused: nothing is decoded or queued; `seek` holds where to go on.
    paused: bool,
    paused_at: f64,
    /// Volume applied to the samples (0.0..=1.0).
    volume: f32,
}

impl Job {
    fn new(req: Request, dev_rate: u32, dev_ch: u16) -> Job {
        let info = req.src.info();
        let src_ch = usize::from(info.channels.max(1));
        Job {
            id: req.id,
            path: req.path,
            rs: Resampler::new(info.sample_rate, dev_rate, src_ch),
            src: req.src,
            info,
            _temp: req._temp,
            seek: None,
            dev_rate: f64::from(dev_rate.max(1)),
            dev_ch: usize::from(dev_ch.max(1)),
            base: 0.0,
            decoded: 0,
            resampled: Vec::new(),
            pending: Vec::new(),
            sent: 0,
            end: None,
            last_tick: None,
            notice: None,
            paused: false,
            paused_at: 0.0,
            volume: 1.0,
        }
    }

    /// A new device: resample for it and continue at `at` seconds.
    fn rebind(&mut self, dev_rate: u32, dev_ch: u16, at: f64) {
        let src_ch = usize::from(self.info.channels.max(1));
        self.rs = Resampler::new(self.info.sample_rate, dev_rate, src_ch);
        self.dev_rate = f64::from(dev_rate.max(1));
        self.dev_ch = usize::from(dev_ch.max(1));
        self.seek = Some(at);
    }

    fn src_rate(&self) -> f64 {
        f64::from(self.info.sample_rate.max(1))
    }

    /// Seconds the listener is at.
    fn position(&self, sink: &dyn Sink) -> f64 {
        let unsent = (self.pending.len() - self.sent) / self.dev_ch;
        let behind = (sink.queued() + unsent) as f64 / self.dev_rate;
        (self.base + self.decoded as f64 / self.src_rate() - behind).max(self.base)
    }

    fn emit(&self, gate: &Gate, state: PlayState, position: f64, message: Option<String>) {
        gate.emit(&StateEvent {
            id: self.id,
            path: self.path.clone(),
            state,
            position,
            duration: self.info.duration_secs,
            message,
            info: (state == PlayState::Playing).then(|| self.info.clone()),
        });
    }

    /// One step of the playback.
    fn step(&mut self, sink: &mut dyn Sink, gate: &Gate) -> Step {
        if let Some(f) = sink.failure() {
            return Step::DeviceLost(f);
        }
        if let Some(t) = self.seek.take() {
            if let Err(e) = self.src.seek(t) {
                self.emit(
                    gate,
                    PlayState::Error,
                    self.base,
                    Some(format!("Stopped: {e}")),
                );
                return Step::Done;
            }
            sink.clear();
            self.rs.reset();
            self.pending.clear();
            self.sent = 0;
            self.end = None;
            self.base = t.max(0.0).min(self.info.duration_secs.unwrap_or(f64::MAX));
            self.decoded = 0;
            self.last_tick = None;
        }
        let pos = self.position(sink);
        if self.last_tick.is_none_or(|t| t.elapsed() >= TICK) {
            self.emit(gate, PlayState::Playing, pos, self.notice.clone());
            self.last_tick = Some(Instant::now());
        }
        if self.sent < self.pending.len() {
            self.sent += sink.push(&self.pending[self.sent..]);
            return if self.sent < self.pending.len() {
                Step::Wait
            } else {
                Step::Busy
            };
        }
        if let Some(how) = &self.end {
            if sink.queued() > 0 {
                return Step::Wait;
            }
            match how {
                None => self.emit(gate, PlayState::Ended, pos, None),
                Some(msg) => self.emit(gate, PlayState::Error, pos, Some(msg.clone())),
            }
            return Step::Done;
        }
        let src_ch = usize::from(self.info.channels.max(1));
        self.resampled.clear();
        match self.src.next_block() {
            Ok(Some(block)) => {
                self.decoded += (block.len() / src_ch) as u64;
                self.rs.process(block, &mut self.resampled);
            }
            Ok(None) => {
                self.rs.flush(&mut self.resampled);
                self.end = Some(None);
            }
            Err(e) => {
                self.rs.flush(&mut self.resampled);
                let at = clock(self.base + self.decoded as f64 / self.src_rate());
                self.end = Some(Some(match e {
                    AudioError::Damaged(_) => format!("Stopped: file is damaged at {at}"),
                    e => format!("Stopped at {at}: {e}"),
                }));
            }
        }
        self.pending.clear();
        self.sent = 0;
        map_channels(&self.resampled, src_ch, self.dev_ch, &mut self.pending);
        if self.volume < 1.0 {
            // Squared: the slider then feels even (loudness follows amplitude roughly so).
            let g = self.volume * self.volume;
            for s in &mut self.pending {
                *s *= g;
            }
        }
        Step::Busy
    }
}

// ---------------------------------------------------------------- commands

/// The folder holding the copy `local` when it is a `<temp root>/<prefix>-<n>/` folder of
/// `vfs` (only such folders are ever removed).
fn temp_copy_dir(local: &Path, temp_root: &Path) -> Option<PathBuf> {
    let dir = local.parent()?;
    (dir.parent() == Some(temp_root)).then(|| dir.to_path_buf())
}

/// F3 on an audio file: plays `path` (a location like the viewer takes), replacing whatever
/// plays. Files inside archives and on servers are copied first (`Loading <name>…`); the
/// copy is removed when the playback ends or is replaced.
#[tauri::command]
pub async fn audio_play(app: AppHandle, path: String) -> Result<Started, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let copy = needs_copy(&path);
        let id = state.audio.begin(&path, copy);
        let local = if copy {
            match watch(&app, &state.vfs, || {
                vfs::materialize(&state.vfs, &path, "audio")
            }) {
                Ok(l) => l,
                Err(e) => {
                    // The raw text goes back: on "password required" the UI asks and retries.
                    if is_password_error(&e) {
                        state.audio.cancel(id, &path);
                    } else {
                        state.audio.fail(id, &path, open_copy_error(&path, &e));
                    }
                    return Err(e);
                }
            }
        } else {
            PathBuf::from(&path)
        };
        let cleanup: Option<Cleanup> = if copy {
            temp_copy_dir(&local, &state.vfs.temp_root).map(|dir| {
                let ctx = state.vfs.clone();
                Box::new(move || ctx.cleanup_dir(&dir)) as Cleanup
            })
        } else {
            None
        };
        state.audio.play(id, &path, &local, cleanup)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The archive needs a (different) password (see `vfs::archive_error_message`).
fn is_password_error(e: &str) -> bool {
    e.starts_with("password required: ") || e.starts_with("wrong password: ")
}

fn open_copy_error(path: &str, e: &str) -> String {
    format!("Cannot play {}: {e}", file_name(path))
}

/// Output devices for Options (empty when there are none or listing failed).
#[tauri::command]
pub async fn audio_outputs() -> Vec<OutputDevice> {
    tauri::async_runtime::spawn_blocking(outputs)
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub fn audio_stop(state: State<'_, AppState>) {
    state.audio.stop();
}

#[tauri::command]
pub fn audio_seek(state: State<'_, AppState>, secs: f64) {
    state.audio.seek(secs);
}

#[tauri::command]
pub fn audio_pause(state: State<'_, AppState>, paused: bool) {
    state.audio.pause(paused);
}

/// Volume 0.0..=1.0.
#[tauri::command]
pub fn audio_volume(state: State<'_, AppState>, volume: f64) {
    state.audio.set_volume(volume);
}

/// The player of the app: events go to the main window.
pub fn app_player(handle: Arc<std::sync::OnceLock<AppHandle>>) -> Player {
    Player::new(
        Arc::new(CpalFactory),
        Arc::new(move |ev: &StateEvent| {
            if let Some(app) = handle.get() {
                if let Err(e) = app.emit_to("main", STATE_EVENT, ev) {
                    eprintln!("fxfm: emit {STATE_EVENT}: {e}");
                }
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn saved_output_is_found_by_id_then_by_name() {
        use super::{pick_output, OutputDevice};
        let d = |name: &str, id: &str| OutputDevice {
            name: name.into(),
            id: Some(id.into()),
        };
        let devs = [
            d("DAC", "coreaudio:a"),
            d("Speakers", "coreaudio:s"),
            d("DAC", "coreaudio:b"),
        ];
        assert_eq!(pick_output(&devs, "coreaudio:b"), Some(2));
        assert_eq!(pick_output(&devs, "DAC"), Some(0));
        assert_eq!(pick_output(&devs, "Speakers"), Some(1));
        assert_eq!(pick_output(&devs, "coreaudio:gone"), None);
    }
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/core/tests/fixtures/audio")
            .join(name)
    }

    #[derive(Default)]
    struct Stats {
        /// Interleaved samples taken.
        pushed: AtomicUsize,
        /// Frames waiting (stalled sinks only).
        queued: AtomicUsize,
        dropped: AtomicBool,
        /// Makes the sink report a failure (device gone).
        fail: AtomicBool,
        /// Makes `push` panic.
        panic: AtomicBool,
    }

    /// A 48 kHz stereo device that never opens real audio. A draining sink plays instantly;
    /// a stalled one fills up and stops taking samples.
    struct FakeSink {
        drains: bool,
        cap_frames: usize,
        stats: Arc<Stats>,
    }

    impl Sink for FakeSink {
        fn rate(&self) -> u32 {
            48000
        }

        fn channels(&self) -> u16 {
            2
        }

        fn push(&mut self, s: &[f32]) -> usize {
            assert!(!self.stats.panic.load(Ordering::SeqCst), "test panic");
            let n = if self.drains {
                s.len()
            } else {
                let room = self.cap_frames - self.stats.queued.load(Ordering::SeqCst);
                let n = (s.len() / 2).min(room);
                self.stats.queued.fetch_add(n, Ordering::SeqCst);
                n * 2
            };
            self.stats.pushed.fetch_add(n, Ordering::SeqCst);
            n
        }

        fn queued(&self) -> usize {
            self.stats.queued.load(Ordering::SeqCst)
        }

        fn clear(&mut self) {
            self.stats.queued.store(0, Ordering::SeqCst);
        }

        fn failure(&self) -> Option<String> {
            self.stats
                .fail
                .load(Ordering::SeqCst)
                .then(|| "device gone".to_string())
        }
    }

    impl Drop for FakeSink {
        fn drop(&mut self) {
            self.stats.dropped.store(true, Ordering::SeqCst);
        }
    }

    struct FakeFactory {
        drains: bool,
        fail: Option<String>,
        /// Named devices that are connected (any other name falls back to the default).
        devices: Vec<String>,
        /// The device name of every open (None = the default).
        asked: Mutex<Vec<Option<String>>>,
        /// What the system would use now (None = "Built-in").
        now: Mutex<Option<String>>,
        sinks: Mutex<Vec<Arc<Stats>>>,
    }

    impl SinkFactory for FakeFactory {
        fn open(&self, device: Option<&str>) -> Result<Opened, String> {
            lock(&self.asked).push(device.map(str::to_string));
            if let Some(f) = &self.fail {
                return Err(f.clone());
            }
            let stats = Arc::new(Stats::default());
            lock(&self.sinks).push(stats.clone());
            Ok(Opened {
                sink: Box::new(FakeSink {
                    drains: self.drains,
                    cap_frames: 4800,
                    stats,
                }),
                fallback: device.is_some_and(|d| !self.devices.iter().any(|x| x == d)),
                device: Some(lock(&self.now).clone().unwrap_or_else(|| "Built-in".into())),
            })
        }

        fn current(&self, _device: Option<&str>) -> Option<String> {
            Some(lock(&self.now).clone().unwrap_or_else(|| "Built-in".into()))
        }
    }

    impl FakeFactory {
        fn sink(&self, i: usize) -> Arc<Stats> {
            lock(&self.sinks)[i].clone()
        }

        fn opened(&self) -> usize {
            lock(&self.sinks).len()
        }

        fn asked(&self) -> Vec<Option<String>> {
            lock(&self.asked).clone()
        }
    }

    type Events = Arc<Mutex<Vec<StateEvent>>>;

    fn player(drains: bool, fail: Option<&str>) -> (Player, Arc<FakeFactory>, Events) {
        let factory = Arc::new(FakeFactory {
            drains,
            fail: fail.map(str::to_string),
            devices: vec!["USB DAC".into(), "Speakers".into()],
            asked: Mutex::new(Vec::new()),
            now: Mutex::new(None),
            sinks: Mutex::new(Vec::new()),
        });
        let events: Events = Arc::default();
        let ev = events.clone();
        let p = Player::new(
            factory.clone(),
            Arc::new(move |e: &StateEvent| lock(&ev).push(e.clone())),
        );
        (p, factory, events)
    }

    fn play(p: &Player, name: &str, cleanup: Option<Cleanup>) -> Result<Started, String> {
        let id = p.begin(name, false);
        p.play(id, name, &fixture(name), cleanup)
    }

    fn wait_until(what: impl Fn() -> bool) {
        let start = Instant::now();
        while !what() {
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_for(events: &Events, what: impl Fn(&StateEvent) -> bool) -> StateEvent {
        wait_until(|| lock(events).iter().any(&what));
        lock(events).iter().find(|e| what(e)).cloned().unwrap()
    }

    fn flag() -> (Arc<AtomicBool>, Option<Cleanup>) {
        let f = Arc::new(AtomicBool::new(false));
        let g = f.clone();
        (f, Some(Box::new(move || g.store(true, Ordering::SeqCst))))
    }

    #[test]
    fn plays_to_the_end_resampled_to_the_device() {
        let (p, factory, events) = player(true, None);
        let (cleaned, cleanup) = flag();
        let started = play(&p, "pcm_s16.wav", cleanup).unwrap();
        assert_eq!((started.info.sample_rate, started.info.channels), (8000, 1));
        let end = wait_for(&events, |e| e.state == PlayState::Ended);
        assert_eq!((end.id, end.path.as_str()), (started.id, "pcm_s16.wav"));
        assert_eq!(end.duration, Some(0.25));
        wait_until(|| cleaned.load(Ordering::SeqCst));
        // 2000 frames at 8 kHz mono -> 12000 frames at 48 kHz stereo.
        let pushed = factory.sink(0).pushed.load(Ordering::SeqCst);
        assert!(pushed.abs_diff(12000 * 2) <= 12, "{pushed}");
        let first = lock(&events)[0].clone();
        assert_eq!(first.state, PlayState::Playing);
        assert_eq!(
            first.info.as_ref().map(|i| i.codec.as_str()),
            Some("PCM 16-bit")
        );
        // The output stays open for the next file.
        assert!(!factory.sink(0).dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn second_play_stops_the_first_on_the_same_output() {
        let (p, factory, events) = player(false, None);
        let (cleaned, cleanup) = flag();
        let a = play(&p, "flac_8k.flac", cleanup).unwrap();
        wait_for(&events, |e| e.state == PlayState::Playing);
        // Stalled device: the first file is still playing.
        std::thread::sleep(Duration::from_millis(30));
        assert!(!cleaned.load(Ordering::SeqCst));
        let b = play(&p, "pcm_s16.wav", None).unwrap();
        assert!(b.id > a.id);
        // play() returns with the first playback gone and its copy removed.
        assert!(cleaned.load(Ordering::SeqCst));
        lock(&events).clear();
        wait_for(&events, |e| e.path == "pcm_s16.wav");
        std::thread::sleep(Duration::from_millis(30));
        assert!(lock(&events)
            .iter()
            .all(|e| e.id == b.id && e.path == "pcm_s16.wav"));
        // Many quick F3 presses: one device open in all.
        for _ in 0..5 {
            play(&p, "pcm_s16.wav", None).unwrap();
        }
        assert_eq!(factory.opened(), 1);
        p.stop();
        let stopped = wait_for(&events, |e| e.state == PlayState::Stopped);
        assert!(stopped.id > b.id);
        assert_eq!(stopped.path, "pcm_s16.wav");
        p.shutdown();
        assert!(factory.sink(0).dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn pause_stops_feeding_and_resume_goes_on() {
        let (p, factory, events) = player(false, None);
        play(&p, "flac_8k.flac", None).unwrap();
        wait_for(&events, |e| e.state == PlayState::Playing);
        p.pause(true);
        let paused = wait_for(&events, |e| e.state == PlayState::Paused);
        let sink = factory.sink(0);
        // The queue is dropped at once and nothing new is queued while paused.
        wait_until(|| sink.queued.load(Ordering::SeqCst) == 0);
        let pushed = sink.pushed.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(sink.pushed.load(Ordering::SeqCst), pushed);
        let before = lock(&events).len();
        p.pause(false);
        wait_until(|| sink.pushed.load(Ordering::SeqCst) > pushed);
        wait_until(|| {
            lock(&events)[before..]
                .iter()
                .any(|e| e.state == PlayState::Playing)
        });
        let e = lock(&events)[before..]
            .iter()
            .find(|e| e.state == PlayState::Playing)
            .cloned()
            .unwrap();
        assert!(e.position + 0.05 >= paused.position, "{e:?} {paused:?}");
        p.shutdown();
    }

    #[test]
    fn a_new_system_default_takes_over_while_playing() {
        let (p, factory, events) = player(false, None);
        play(&p, "flac_8k.flac", None).unwrap();
        wait_for(&events, |e| e.state == PlayState::Playing);
        assert_eq!(factory.opened(), 1);
        *lock(&factory.now) = Some("AirPods".into());
        wait_until(|| factory.opened() == 2);
        p.shutdown();
    }

    #[test]
    fn volume_is_clamped() {
        let (p, _factory, _events) = player(true, None);
        p.set_volume(0.5);
        assert!((p.output.volume() - 0.5).abs() < 1e-6);
        p.set_volume(7.0);
        assert!((p.output.volume() - 1.0).abs() < 1e-6);
        p.set_volume(-1.0);
        assert!(p.output.volume().abs() < 1e-6);
        p.set_volume(f64::NAN);
        assert!((p.output.volume() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn seek_moves_the_position() {
        let (p, _factory, events) = player(false, None);
        play(&p, "flac_8k.flac", None).unwrap();
        wait_for(&events, |e| e.state == PlayState::Playing);
        p.seek(0.6);
        let e = wait_for(&events, |e| e.position >= 0.6);
        assert!(e.position < 0.7, "{e:?}");
        p.shutdown();
    }

    #[test]
    fn unsupported_codec_is_reported() {
        let (p, factory, events) = player(true, None);
        let (cleaned, cleanup) = flag();
        let err = play(&p, "g726.wav", cleanup).unwrap_err();
        assert_eq!(
            err,
            "Cannot play g726.wav: unsupported codec G.726 (WAV format tag 0x0045)"
        );
        assert!(cleaned.load(Ordering::SeqCst));
        assert_eq!(factory.opened(), 0);
        let ev = lock(&events).last().cloned().unwrap();
        assert_eq!((ev.state, ev.message), (PlayState::Error, Some(err)));
    }

    #[test]
    fn no_output_device_is_reported() {
        let (p, _factory, events) = player(true, Some(NO_DEVICE));
        let (cleaned, cleanup) = flag();
        assert_eq!(play(&p, "pcm_s16.wav", cleanup).unwrap_err(), NO_DEVICE);
        assert!(cleaned.load(Ordering::SeqCst));
        let ev = lock(&events).last().cloned().unwrap();
        assert_eq!(ev.message.as_deref(), Some(NO_DEVICE));
    }

    #[test]
    fn lost_device_is_reopened() {
        let (p, factory, events) = player(false, None);
        play(&p, "flac_8k.flac", None).unwrap();
        wait_for(&events, |e| e.state == PlayState::Playing);
        factory.sink(0).fail.store(true, Ordering::SeqCst);
        wait_until(|| factory.opened() == 2);
        wait_until(|| factory.sink(1).pushed.load(Ordering::SeqCst) > 0);
        assert!(factory.sink(0).dropped.load(Ordering::SeqCst));
        assert!(lock(&events).iter().all(|e| e.state == PlayState::Playing));
        p.shutdown();
    }

    #[test]
    fn panic_while_playing_is_reported_and_cleaned_up() {
        let (p, factory, events) = player(true, None);
        let a = play(&p, "flac_8k.flac", None).unwrap();
        // Armed only after the first file ended: armed earlier, its own last pushes may panic.
        wait_for(&events, |e| e.id == a.id && e.state == PlayState::Ended);
        // The next file's first push panics.
        factory.sink(0).panic.store(true, Ordering::SeqCst);
        let (cleaned, cleanup) = flag();
        let b = play(&p, "pcm_s16.wav", cleanup).unwrap();
        let e = wait_for(&events, |e| e.state == PlayState::Error);
        assert_eq!(e.id, b.id);
        assert_eq!(
            e.message.as_deref(),
            Some("Cannot play pcm_s16.wav: file is damaged")
        );
        assert!(cleaned.load(Ordering::SeqCst));
        // The audio thread survives.
        factory.sink(0).panic.store(false, Ordering::SeqCst);
        play(&p, "pcm_s16.wav", None).unwrap();
        wait_for(&events, |e| e.state == PlayState::Ended);
    }

    #[test]
    fn damaged_file_plays_then_reports_where() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = std::fs::read(fixture("gsm_wav49.wav")).unwrap();
        // 12 whole WAV49 blocks + part of one: 0.48 s good.
        let p_file = dir.path().join("cut.wav");
        std::fs::write(&p_file, &bytes[..60 + 65 * 12 + 30]).unwrap();
        let (p, factory, events) = player(true, None);
        let id = p.begin("cut.wav", false);
        p.play(id, "cut.wav", &p_file, None).unwrap();
        let e = wait_for(&events, |e| e.state == PlayState::Error);
        assert_eq!(
            e.message.as_deref(),
            Some("Stopped: file is damaged at 0:00")
        );
        let pushed = factory.sink(0).pushed.load(Ordering::SeqCst);
        assert!(pushed.abs_diff(12 * 320 * 6 * 2) <= 12, "{pushed}");
    }

    #[test]
    fn superseded_request_does_not_play() {
        let (p, factory, events) = player(true, None);
        let first = p.begin("a.zip/x.wav", true);
        let loading = lock(&events)[0].clone();
        assert_eq!((loading.state, loading.id), (PlayState::Loading, first));
        let _second = p.begin("pcm_s16.wav", false);
        let (cleaned, cleanup) = flag();
        let r = p.play(first, "a.zip/x.wav", &fixture("pcm_s16.wav"), cleanup);
        assert_eq!(r.unwrap_err(), SUPERSEDED);
        assert!(cleaned.load(Ordering::SeqCst));
        assert_eq!(factory.opened(), 0);
        // Stop while loading: the strip hears `stopped` for the loading file.
        let third = p.begin("srv/y.wav", true);
        p.stop();
        assert_eq!(
            p.play(third, "srv/y.wav", &fixture("pcm_s16.wav"), None)
                .unwrap_err(),
            SUPERSEDED
        );
        let last = lock(&events).last().cloned().unwrap();
        assert_eq!(
            (last.state, last.path.as_str()),
            (PlayState::Stopped, "srv/y.wav")
        );
    }

    #[test]
    fn wanted_device_reaches_the_factory() {
        let (p, factory, events) = player(true, None);
        p.set_output(Some("USB DAC".into()));
        play(&p, "pcm_s16.wav", None).unwrap();
        wait_for(&events, |e| e.state == PlayState::Ended);
        assert_eq!(factory.asked(), vec![Some("USB DAC".to_string())]);
        assert!(lock(&events).iter().all(|e| e.message.is_none()));
        // An empty name is the system default.
        p.set_output(Some(String::new()));
        play(&p, "pcm_s16.wav", None).unwrap();
        assert_eq!(factory.asked().last(), Some(&None));
        p.shutdown();
    }

    #[test]
    fn missing_device_falls_back_and_says_so_once() {
        let (p, factory, events) = player(false, None);
        p.set_output(Some("Headphones".into()));
        play(&p, "flac_8k.flac", None).unwrap();
        let note = "Output: Headphones is not connected, using the system default";
        let first = wait_for(&events, |e| e.state == PlayState::Playing);
        assert_eq!(first.message.as_deref(), Some(note));
        // The device fails and is reopened (still missing): no second notice.
        factory.sink(0).fail.store(true, Ordering::SeqCst);
        wait_until(|| factory.opened() == 2);
        lock(&events).clear();
        wait_for(&events, |e| e.state == PlayState::Playing);
        assert!(lock(&events).iter().all(|e| e.message.is_none()));
        // Nor after the audio thread is restarted.
        p.shutdown();
        lock(&events).clear();
        play(&p, "pcm_s16.wav", None).unwrap();
        wait_for(&events, |e| e.state == PlayState::Playing);
        assert_eq!(factory.opened(), 3);
        assert!(lock(&events).iter().all(|e| e.message.is_none()));
        assert_eq!(factory.asked(), vec![Some("Headphones".to_string()); 3]);
        p.shutdown();
    }

    #[test]
    fn changing_the_device_between_plays_opens_the_new_one() {
        let (p, factory, events) = player(true, None);
        play(&p, "pcm_s16.wav", None).unwrap();
        wait_for(&events, |e| e.state == PlayState::Ended);
        p.set_output(Some("Speakers".into()));
        play(&p, "pcm_s16.wav", None).unwrap();
        assert_eq!(factory.asked(), vec![None, Some("Speakers".to_string())]);
        assert!(factory.sink(0).dropped.load(Ordering::SeqCst));
        // The same choice again keeps the open device.
        p.set_output(Some("Speakers".into()));
        play(&p, "pcm_s16.wav", None).unwrap();
        assert_eq!(factory.opened(), 2);
        p.shutdown();
    }

    #[test]
    fn changing_the_device_while_playing_moves_the_playback() {
        let (p, factory, events) = player(false, None);
        let started = play(&p, "flac_8k.flac", None).unwrap();
        wait_for(&events, |e| e.state == PlayState::Playing);
        p.seek(0.6);
        wait_for(&events, |e| e.position >= 0.6);
        p.set_output(Some("USB DAC".into()));
        wait_until(|| factory.opened() == 2);
        wait_until(|| factory.sink(1).pushed.load(Ordering::SeqCst) > 0);
        assert!(factory.sink(0).dropped.load(Ordering::SeqCst));
        assert_eq!(factory.asked(), vec![None, Some("USB DAC".to_string())]);
        // Same playback, same place, nothing but `playing`.
        lock(&events).clear();
        let e = wait_for(&events, |e| e.state == PlayState::Playing);
        assert_eq!(e.id, started.id);
        assert!(e.position >= 0.6, "{e:?}");
        assert!(lock(&events).iter().all(|e| e.state == PlayState::Playing));
        p.shutdown();
    }

    #[test]
    fn only_vfs_temp_folders_are_removed() {
        let root = Path::new("/c/fxfm/tmp/123");
        assert_eq!(
            temp_copy_dir(&root.join("audio-4/x.wav"), root),
            Some(root.join("audio-4"))
        );
        assert_eq!(temp_copy_dir(Path::new("/Users/me/rec/x.wav"), root), None);
        assert_eq!(temp_copy_dir(&root.join("x.wav"), root), None);
        assert_eq!(temp_copy_dir(&root.join("audio-4/sub/x.wav"), root), None);
    }

    fn sine(rate: u32, freq: f64, n: usize, offset: usize) -> Vec<f32> {
        (offset..offset + n)
            .map(|i| {
                (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate)).sin())
                    as f32
            })
            .collect()
    }

    #[test]
    fn resampler_8k_to_48k_keeps_the_signal() {
        let mut r = Resampler::new(8000, 48000, 1);
        let input = sine(8000, 1000.0, 8000, 0);
        let mut out = Vec::new();
        // In uneven pieces, like decoder blocks.
        for chunk in input.chunks(333) {
            r.process(chunk, &mut out);
        }
        r.flush(&mut out);
        assert!(out.len().abs_diff(48000) <= 6, "{}", out.len());
        // Away from the edges it is the same sine at 48 kHz.
        let want = sine(48000, 1000.0, 40000, 4000);
        let err = out[4000..44000]
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(err < 0.01, "max error {err}");
    }

    #[test]
    fn resampler_downsamples_without_aliasing() {
        // 20 kHz at 48 kHz is above the new Nyquist (11.025 kHz) and must vanish.
        let mut r = Resampler::new(48000, 22050, 1);
        let mut out = Vec::new();
        r.process(&sine(48000, 20000.0, 48000, 0), &mut out);
        let mid = &out[2000..20000];
        let rms = (mid.iter().map(|x| x * x).sum::<f32>() / mid.len() as f32).sqrt();
        assert!(rms < 0.01, "rms {rms}");
        // Same rate passes through untouched.
        let mut same = Resampler::new(44100, 44100, 2);
        let mut out = Vec::new();
        same.process(&[0.1, 0.2, 0.3, 0.4], &mut out);
        assert_eq!(out, vec![0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn resampler_table_is_capped() {
        // A hostile rate must not size the filter table.
        let r = Resampler::new(u32::MAX, 8000, 1);
        assert!(r.table.len() <= (PHASES + 1) * 2048, "{}", r.table.len());
    }

    #[test]
    fn channel_mapping() {
        let mut out = Vec::new();
        map_channels(&[0.5, -0.5], 1, 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
        out.clear();
        map_channels(&[0.2, 0.4], 2, 1, &mut out);
        assert!((out[0] - 0.3).abs() < 1e-6);
        out.clear();
        map_channels(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 6, 2, &mut out);
        assert_eq!(out, vec![1.0, 2.0]);
    }

    #[test]
    fn names_for_messages() {
        assert_eq!(file_name("/a/b/c.wav"), "c.wav");
        assert_eq!(file_name("sftp://srv/rec/x.gsm"), "x.gsm");
        assert_eq!(file_name("/a/calls.zip/in/y.wav"), "y.wav");
    }
}
