//! Audio decoding for F3 audition: a file becomes a stream of interleaved f32 blocks.
//!
//! Containers and codecs come from symphonia (pure Rust) plus `symphonia-opus`; GSM 06.10
//! (WAV49 and raw `.gsm`) and Sun/NeXT `.au` are decoded here. Nothing here plays sound: the
//! output lives in the app (`src-tauri/src/audio.rs`).

pub mod gsm;
mod pcm;

use serde::Serialize;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use symphonia::core::codecs::audio::well_known::*;
use symphonia::core::codecs::audio::{AudioCodecId, AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::common::Tier;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

/// What the UI shows about a stream.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInfo {
    /// Human-readable codec, e.g. `GSM 6.10` or `MP3`.
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AudioError {
    /// The codec (or WAV format tag) has no decoder; the text names it.
    #[error("unsupported codec {0}")]
    Unsupported(String),
    /// The data is broken; what decoded so far is good.
    #[error("file is damaged ({0})")]
    Damaged(String),
    #[error("{0}")]
    Io(String),
}

fn io_err(e: io::Error) -> AudioError {
    AudioError::Io(e.to_string())
}

/// A decoded stream.
pub trait AudioSource: Send {
    fn info(&self) -> AudioInfo;
    /// The next block of interleaved samples (`info().channels` per frame); None at the end.
    fn next_block(&mut self) -> Result<Option<&[f32]>, AudioError>;
    /// Moves so that the next block starts at `secs` (clamped to the stream).
    fn seek(&mut self, secs: f64) -> Result<(), AudioError>;
}

/// Highest sample rate a header may claim (anything above is a broken or hostile header).
pub const MAX_SAMPLE_RATE: u32 = 768_000;
/// Most channels a header may claim.
pub const MAX_CHANNELS: u16 = 64;

/// Opens `path` for decoding. Only headers are read: decoding happens block by block.
/// A source always has 1..=MAX_SAMPLE_RATE Hz and 1..=MAX_CHANNELS channels.
pub fn open(path: &Path) -> Result<Box<dyn AudioSource>, AudioError> {
    let src = open_any(path)?;
    let info = src.info();
    check_format(info.sample_rate, u32::from(info.channels))?;
    Ok(src)
}

/// Refuses sample rates and channel counts no real file has.
fn check_format(rate: u32, channels: u32) -> Result<(), AudioError> {
    if rate == 0 || rate > MAX_SAMPLE_RATE {
        return Err(AudioError::Damaged(format!(
            "bad header: sample rate {rate} Hz"
        )));
    }
    if channels == 0 || channels > u32::from(MAX_CHANNELS) {
        return Err(AudioError::Damaged(format!(
            "bad header: {channels} channels"
        )));
    }
    Ok(())
}

fn open_any(path: &Path) -> Result<Box<dyn AudioSource>, AudioError> {
    let mut f = File::open(path).map_err(io_err)?;
    let mut head = [0u8; 12];
    let n = read_full(&mut f, &mut head).map_err(io_err)?;
    f.seek(SeekFrom::Start(0)).map_err(io_err)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if n == 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WAVE" {
        return open_wav(path, f);
    }
    if n >= 4 && &head[0..4] == b".snd" {
        return pcm::open_au(f);
    }
    if ext.as_deref() == Some("gsm") {
        return GsmSource::raw(f);
    }
    let len = f.metadata().map_err(io_err)?.len();
    open_symphonia(Input::new(path, 0, len, ext.as_deref()))
}

/// Reads until `buf` is full or the end; returns the bytes read.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// `secs` as `m:ss` (minutes not wrapped).
pub fn clock(secs: f64) -> String {
    let s = if secs.is_finite() && secs > 0.0 {
        secs.floor() as u64
    } else {
        0
    };
    format!("{}:{:02}", s / 60, s % 60)
}

// ---------------------------------------------------------------- WAV

pub const WAVE_FORMAT_PCM: u16 = 0x0001;
pub const WAVE_FORMAT_MS_ADPCM: u16 = 0x0002;
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
pub const WAVE_FORMAT_ALAW: u16 = 0x0006;
pub const WAVE_FORMAT_MULAW: u16 = 0x0007;
pub const WAVE_FORMAT_IMA_ADPCM: u16 = 0x0011;
pub const WAVE_FORMAT_GSM610: u16 = 0x0031;
pub const WAVE_FORMAT_G726: u16 = 0x0045;
pub const WAVE_FORMAT_MPEGLAYER3: u16 = 0x0055;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Names of WAV format tags met in telephony and on old PCs (none of these is decoded).
fn wav_tag_name(tag: u16) -> Option<&'static str> {
    Some(match tag {
        0x0010 => "OKI ADPCM",
        0x0017 => "Dialogic OKI ADPCM",
        0x0020 => "Yamaha ADPCM",
        0x0022 => "DSP Group TrueSpeech",
        0x0028 => "LRC",
        0x0030 => "Dolby AC-2",
        0x0040 => "G.721",
        0x0042 => "G.723.1",
        0x0045 => "G.726",
        0x0050 => "MPEG audio",
        0x0064 => "G.726 (APICOM)",
        0x0065 => "G.722",
        0x0067 => "G.722.1",
        0x0070 => "Lernout & Hauspie CELP",
        0x0083 => "G.729",
        0x0092 => "Dolby AC-3 (SPDIF)",
        0x0130 => "ACELP.net",
        0x0160 => "Windows Media Audio 1",
        0x0161 => "Windows Media Audio",
        0x0162 => "Windows Media Audio Pro",
        0x0163 => "Windows Media Audio Lossless",
        0x00FF => "AAC",
        0x1600 => "AAC (ADTS)",
        0x2000 => "Dolby AC-3",
        0x6C75 => "mu-law",
        _ => return None,
    })
}

/// The codec text of an unsupported WAV format tag: `G.726 (WAV format tag 0x0045)`.
fn wav_tag_label(tag: u16) -> String {
    match wav_tag_name(tag) {
        Some(n) => format!("{n} (WAV format tag 0x{tag:04X})"),
        None => format!("WAV format tag 0x{tag:04X}"),
    }
}

/// What a WAV header says.
#[derive(Debug, Default)]
struct WavHeader {
    /// Format tag; for WAVE_FORMAT_EXTENSIBLE the sub-format's.
    tag: u16,
    extensible: bool,
    channels: u16,
    rate: u32,
    block_align: u16,
    /// Frames per channel from the `fact` chunk.
    fact: Option<u32>,
    data_start: u64,
    /// As declared (may run past the end of a truncated file).
    data_len: u64,
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Walks the RIFF chunks up to `data`.
fn read_wav_header(f: &mut File) -> Result<WavHeader, AudioError> {
    let damaged = |what: &str| AudioError::Damaged(format!("WAV header: {what}"));
    f.seek(SeekFrom::Start(12)).map_err(io_err)?;
    let mut h = WavHeader::default();
    let mut have_fmt = false;
    let mut pos = 12u64;
    loop {
        let mut ch = [0u8; 8];
        if read_full(f, &mut ch).map_err(io_err)? < 8 {
            return Err(damaged("no data chunk"));
        }
        let size = u64::from(le32(&ch, 4));
        pos += 8;
        match &ch[0..4] {
            b"fmt " => {
                let mut b = vec![0u8; size.min(64) as usize];
                if size < 16 || read_full(f, &mut b).map_err(io_err)? < b.len() {
                    return Err(damaged("short fmt chunk"));
                }
                h.tag = le16(&b, 0);
                h.channels = le16(&b, 2);
                h.rate = le32(&b, 4);
                h.block_align = le16(&b, 12);
                if h.tag == WAVE_FORMAT_EXTENSIBLE {
                    if b.len() < 26 {
                        return Err(damaged("short WAVE_FORMAT_EXTENSIBLE"));
                    }
                    h.extensible = true;
                    h.tag = le16(&b, 24);
                }
                have_fmt = true;
            }
            b"fact" if size >= 4 => {
                let mut b = [0u8; 4];
                if read_full(f, &mut b).map_err(io_err)? == 4 {
                    h.fact = Some(le32(&b, 0));
                }
            }
            b"data" => {
                if !have_fmt {
                    return Err(damaged("data before fmt"));
                }
                h.data_start = pos;
                h.data_len = size;
                return Ok(h);
            }
            _ => {}
        }
        pos += size + (size & 1);
        f.seek(SeekFrom::Start(pos)).map_err(io_err)?;
    }
}

fn open_wav(path: &Path, mut f: File) -> Result<Box<dyn AudioSource>, AudioError> {
    let mut h = read_wav_header(&mut f)?;
    check_format(h.rate, u32::from(h.channels))?;
    if h.block_align == 0 {
        return Err(AudioError::Damaged("bad header: block align 0".into()));
    }
    let file_len = f.metadata().map_err(io_err)?.len();
    let in_file = file_len.saturating_sub(h.data_start);
    // A recorder that never finished its header leaves the data size 0 or 0xFFFFFFFF: the data
    // then runs to the end of the file.
    if h.data_len == 0 || h.data_len == u64::from(u32::MAX) {
        h.data_len = in_file;
    }
    match h.tag {
        WAVE_FORMAT_GSM610 => GsmSource::wav49(f, &h),
        WAVE_FORMAT_MPEGLAYER3 => {
            let data_len = h.data_len.min(in_file);
            open_symphonia(Input {
                truncated: data_len < h.data_len,
                ..Input::new(path, h.data_start, data_len, Some("mp3"))
            })
        }
        WAVE_FORMAT_ALAW | WAVE_FORMAT_MULAW => pcm::g711_wav(f, h.tag == WAVE_FORMAT_ALAW, &h),
        WAVE_FORMAT_PCM | WAVE_FORMAT_MS_ADPCM | WAVE_FORMAT_IEEE_FLOAT | WAVE_FORMAT_IMA_ADPCM => {
            // symphonia reads the RIFF header too. It refuses sizes that run past the end of
            // the file, so it gets a copy of the header with the sizes of what is there.
            let data_len = h.data_len.min(in_file);
            let input = Input {
                prefix: repaired_header(&mut f, &h, data_len)?,
                truncated: data_len < h.data_len,
                ..Input::new(path, h.data_start, data_len, Some("wav"))
            };
            // An unsupported extensible sub-type is reported with its tag.
            open_symphonia(input).map_err(|e| match e {
                AudioError::Unsupported(_) if h.extensible => {
                    AudioError::Unsupported(format!("WAV extensible sub-format 0x{:04X}", h.tag))
                }
                e => e,
            })
        }
        tag => Err(AudioError::Unsupported(wav_tag_label(tag))),
    }
}

/// The file's bytes up to the `data` payload, with the RIFF and `data` sizes set for a data
/// chunk of `data_len` bytes (the chunks after it are left out).
fn repaired_header(f: &mut File, h: &WavHeader, data_len: u64) -> Result<Arc<[u8]>, AudioError> {
    // The header walk stops at `data`; it is small unless the file is hostile.
    if h.data_start > 1 << 20 {
        return Err(AudioError::Damaged(
            "bad header: 1 MB of chunks before data".into(),
        ));
    }
    let mut head = vec![0u8; h.data_start as usize];
    f.seek(SeekFrom::Start(0)).map_err(io_err)?;
    if read_full(f, &mut head).map_err(io_err)? < head.len() {
        return Err(AudioError::Damaged("short WAV header".into()));
    }
    let riff = (head.len() as u64 + data_len - 8).min(u64::from(u32::MAX)) as u32;
    head[4..8].copy_from_slice(&riff.to_le_bytes());
    let at = head.len() - 4;
    head[at..].copy_from_slice(&(data_len as u32).to_le_bytes());
    Ok(Arc::from(head))
}

// ---------------------------------------------------------------- byte region

/// A byte range of a file as a symphonia media source (the `data` chunk of MP3-in-WAV),
/// optionally behind in-memory bytes (a repaired WAV header).
struct Region {
    prefix: Arc<[u8]>,
    f: BufReader<File>,
    start: u64,
    /// Bytes of the whole source (prefix included).
    len: u64,
    pos: u64,
}

impl Region {
    fn new(f: File, prefix: Arc<[u8]>, start: u64, file_len: u64) -> Region {
        let mut f = BufReader::with_capacity(64 * 1024, f);
        let _ = f.seek(SeekFrom::Start(start));
        Region {
            len: prefix.len() as u64 + file_len,
            prefix,
            f,
            start,
            pos: 0,
        }
    }
}

impl Read for Region {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let p = self.prefix.len() as u64;
        if self.pos < p {
            let from = &self.prefix[self.pos as usize..];
            let n = from.len().min(buf.len());
            buf[..n].copy_from_slice(&from[..n]);
            self.pos += n as u64;
            if self.pos == p {
                self.f.seek(SeekFrom::Start(self.start))?;
            }
            return Ok(n);
        }
        let left = self.len.saturating_sub(self.pos);
        let n = buf.len().min(left.min(usize::MAX as u64) as usize);
        if n == 0 {
            return Ok(0);
        }
        let k = self.f.read(&mut buf[..n])?;
        self.pos += k as u64;
        Ok(k)
    }
}

impl Seek for Region {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        let target = target as u64;
        let p = self.prefix.len() as u64;
        self.f
            .seek(SeekFrom::Start(self.start + target.saturating_sub(p)))?;
        self.pos = target;
        Ok(target)
    }
}

impl MediaSource for Region {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

// ---------------------------------------------------------------- symphonia

fn codecs() -> &'static CodecRegistry {
    static REG: OnceLock<CodecRegistry> = OnceLock::new();
    REG.get_or_init(|| {
        let mut r = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut r);
        r.register_audio_decoder_at_tier::<symphonia_opus::OpusDecoder>(Tier::Fallback);
        r
    })
}

/// Display name of a symphonia codec.
fn codec_name(id: AudioCodecId) -> String {
    let s = match id {
        CODEC_ID_PCM_U8 | CODEC_ID_PCM_S8 => "PCM 8-bit",
        CODEC_ID_PCM_S16LE | CODEC_ID_PCM_S16BE | CODEC_ID_PCM_U16LE | CODEC_ID_PCM_U16BE => {
            "PCM 16-bit"
        }
        CODEC_ID_PCM_S24LE | CODEC_ID_PCM_S24BE | CODEC_ID_PCM_U24LE | CODEC_ID_PCM_U24BE => {
            "PCM 24-bit"
        }
        CODEC_ID_PCM_S32LE | CODEC_ID_PCM_S32BE | CODEC_ID_PCM_U32LE | CODEC_ID_PCM_U32BE => {
            "PCM 32-bit"
        }
        CODEC_ID_PCM_F32LE | CODEC_ID_PCM_F32BE => "PCM float 32-bit",
        CODEC_ID_PCM_F64LE | CODEC_ID_PCM_F64BE => "PCM float 64-bit",
        CODEC_ID_PCM_ALAW => "A-law",
        CODEC_ID_PCM_MULAW => "mu-law",
        CODEC_ID_ADPCM_MS => "MS ADPCM",
        CODEC_ID_ADPCM_IMA_WAV | CODEC_ID_ADPCM_IMA_QT => "IMA ADPCM",
        CODEC_ID_ADPCM_G722 => "G.722",
        CODEC_ID_ADPCM_G726 | CODEC_ID_ADPCM_G726LE => "G.726",
        CODEC_ID_MP1 => "MP1",
        CODEC_ID_MP2 => "MP2",
        CODEC_ID_MP3 => "MP3",
        CODEC_ID_AAC => "AAC",
        CODEC_ID_VORBIS => "Vorbis",
        CODEC_ID_OPUS => "Opus",
        CODEC_ID_FLAC => "FLAC",
        CODEC_ID_ALAC => "ALAC",
        CODEC_ID_SPEEX => "Speex",
        CODEC_ID_AC3 => "AC-3",
        CODEC_ID_EAC3 => "E-AC-3",
        CODEC_ID_WMA => "Windows Media Audio",
        CODEC_ID_WAVPACK => "WavPack",
        CODEC_ID_MONKEYS_AUDIO => "Monkey's Audio",
        CODEC_ID_TTA => "TTA",
        _ => return format!("codec id {id}"),
    };
    s.to_string()
}

fn sym_err(e: SymError) -> AudioError {
    match e {
        SymError::IoError(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
            AudioError::Damaged("unexpected end of file".into())
        }
        SymError::IoError(e) => AudioError::Io(e.to_string()),
        SymError::Unsupported(what) => AudioError::Unsupported(what.to_string()),
        e => AudioError::Damaged(e.to_string()),
    }
}

/// Decode errors in a row before the stream counts as damaged (single bad packets are skipped).
const MAX_DECODE_ERRORS: u32 = 20;

struct SymSource {
    input: Input,
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    info: AudioInfo,
    buf: Vec<f32>,
    /// Interleaved samples to drop from the next blocks (accurate seek).
    skip: usize,
}

/// What symphonia reads: a byte range of a file, with the extension as a format hint.
#[derive(Clone)]
struct Input {
    path: PathBuf,
    /// Read before the file range (a repaired header).
    prefix: Arc<[u8]>,
    /// The header promised more data than the file holds: the end is reported as damage.
    truncated: bool,
    start: u64,
    len: u64,
    ext: Option<String>,
}

impl Input {
    fn new(path: &Path, start: u64, len: u64, ext: Option<&str>) -> Input {
        Input {
            path: path.to_path_buf(),
            prefix: Arc::from(Vec::new()),
            truncated: false,
            start,
            len,
            ext: ext.map(str::to_string),
        }
    }

    fn probe(&self) -> Result<Box<dyn FormatReader>, AudioError> {
        let f = File::open(&self.path).map_err(io_err)?;
        let mut hint = Hint::new();
        if let Some(e) = &self.ext {
            hint.with_extension(e);
        }
        let src = Region::new(f, self.prefix.clone(), self.start, self.len);
        let mss = MediaSourceStream::new(Box::new(src), Default::default());
        symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| match e {
                SymError::Unsupported(_) => AudioError::Unsupported("(unknown file format)".into()),
                e => sym_err(e),
            })
    }
}

fn open_symphonia(input: Input) -> Result<Box<dyn AudioSource>, AudioError> {
    let reader = input.probe()?;
    let track = reader
        .default_track(TrackType::Audio)
        .ok_or_else(|| AudioError::Unsupported("(no audio track)".into()))?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| AudioError::Unsupported("(no audio track)".into()))?
        .clone();
    let codec = codec_name(params.codec);
    let decoder = codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| match e {
            SymError::Unsupported(_) => AudioError::Unsupported(codec.clone()),
            e => sym_err(e),
        })?;
    let rate = params.sample_rate.unwrap_or(0);
    let channels = params.channels.as_ref().map_or(0, |c| c.count()) as u16;
    if rate == 0 || channels == 0 {
        return Err(AudioError::Damaged("no sample rate or channels".into()));
    }
    let time_base = track.time_base;
    let duration_secs = match (track.num_frames, track.duration, time_base) {
        (Some(n), _, _) => Some(n as f64 / f64::from(rate)),
        (None, Some(d), Some(tb)) => tb.calc_duration(d).map(|t| t.as_secs_f64()),
        _ => None,
    };
    let track_id = track.id;
    Ok(Box::new(SymSource {
        input,
        reader,
        decoder,
        track_id,
        time_base,
        info: AudioInfo {
            codec,
            sample_rate: rate,
            channels,
            duration_secs,
        },
        buf: Vec::new(),
        skip: 0,
    }))
}

impl AudioSource for SymSource {
    fn info(&self) -> AudioInfo {
        self.info.clone()
    }

    fn next_block(&mut self) -> Result<Option<&[f32]>, AudioError> {
        let mut errors = 0;
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) if self.input.truncated => {
                    return Err(AudioError::Damaged("unexpected end of file".into()))
                }
                Ok(None) | Err(SymError::ResetRequired) => return Ok(None),
                Err(e) => return Err(sym_err(e)),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymError::DecodeError(what)) => {
                    errors += 1;
                    if errors >= MAX_DECODE_ERRORS {
                        return Err(AudioError::Damaged(what.into()));
                    }
                    continue;
                }
                Err(e) => return Err(sym_err(e)),
            };
            errors = 0;
            let got = decoded.spec().channels().count();
            decoded.copy_to_vec_interleaved(&mut self.buf);
            let want = usize::from(self.info.channels);
            if got != want && got > 0 {
                self.buf = remap(&self.buf, got, want);
            }
            if self.skip > 0 {
                let n = self.skip.min(self.buf.len());
                self.buf.drain(..n);
                self.skip -= n;
            }
            if !self.buf.is_empty() {
                return Ok(Some(&self.buf));
            }
        }
    }

    fn seek(&mut self, secs: f64) -> Result<(), AudioError> {
        let secs = clamp_secs(secs, self.info.duration_secs);
        // A fresh reader each time: symphonia 0.6.1's FLAC reader cannot seek back into the
        // first frames once it reached the end.
        self.reader = self.input.probe()?;
        self.decoder.reset();
        self.skip = 0;
        if secs == 0.0 {
            return Ok(());
        }
        let time = Time::try_from_secs_f64(secs).unwrap_or_default();
        let to = SeekTo::Time {
            time,
            track_id: Some(self.track_id),
        };
        let seeked = self
            .reader
            .seek(SeekMode::Accurate, to)
            .map_err(|e| AudioError::Io(format!("seek: {e}")))?;
        self.decoder.reset();
        let frames = ts_frames(
            seeked.required_ts,
            seeked.actual_ts,
            self.time_base,
            self.info.sample_rate,
        );
        self.skip = frames * usize::from(self.info.channels);
        Ok(())
    }
}

/// Frames between two track timestamps.
fn ts_frames(required: Timestamp, actual: Timestamp, tb: Option<TimeBase>, rate: u32) -> usize {
    let (r, a) = (required.get(), actual.get());
    if r <= a {
        return 0;
    }
    let secs = match tb {
        Some(tb) => {
            let t = |ts| tb.calc_time(ts).map_or(0.0, |t: Time| t.as_secs_f64());
            t(required) - t(actual)
        }
        None => return (r - a) as usize,
    };
    (secs * f64::from(rate)).round().max(0.0) as usize
}

fn clamp_secs(secs: f64, duration: Option<f64>) -> f64 {
    let secs = if secs.is_finite() { secs.max(0.0) } else { 0.0 };
    match duration {
        Some(d) if secs > d => d,
        _ => secs,
    }
}

/// Interleaved `from`-channel samples as `to` channels (first channels kept, mono copied).
fn remap(s: &[f32], from: usize, to: usize) -> Vec<f32> {
    let frames = s.len() / from;
    let mut out = Vec::with_capacity(frames * to);
    for f in s.chunks_exact(from) {
        for c in 0..to {
            out.push(if from == 1 {
                f[0]
            } else {
                f.get(c).copied().unwrap_or(0.0)
            });
        }
    }
    out
}

// ---------------------------------------------------------------- GSM 06.10

struct GsmSource {
    f: BufReader<File>,
    wav49: bool,
    data_start: u64,
    /// Data bytes (WAV49: from the header; raw: the file).
    data_len: u64,
    /// Samples to play in all (WAV49 `fact`); None plays whole frames.
    total: Option<u64>,
    /// Samples decoded so far, counted from the start.
    pos: u64,
    /// Data bytes read so far.
    consumed: u64,
    dec: gsm::Decoder,
    pcm: [i16; gsm::WAV49_BLOCK_SAMPLES],
    out: Vec<f32>,
    skip: usize,
    info: AudioInfo,
}

impl GsmSource {
    fn raw(f: File) -> Result<Box<dyn AudioSource>, AudioError> {
        let len = f.metadata().map_err(io_err)?.len();
        Ok(Box::new(GsmSource::new(f, false, 0, len, None, 8000)))
    }

    fn wav49(f: File, h: &WavHeader) -> Result<Box<dyn AudioSource>, AudioError> {
        if h.channels != 1 {
            return Err(AudioError::Unsupported(format!(
                "GSM 6.10 with {} channels",
                h.channels
            )));
        }
        let rate = h.rate;
        let fact = h.fact.map(u64::from);
        Ok(Box::new(GsmSource::new(
            f,
            true,
            h.data_start,
            h.data_len,
            fact,
            rate,
        )))
    }

    fn new(f: File, wav49: bool, start: u64, len: u64, total: Option<u64>, rate: u32) -> Self {
        let (unit_bytes, unit_samples) = unit(wav49);
        let whole = len / unit_bytes * unit_samples;
        let samples = total.map_or(whole, |t| t.min(whole));
        let mut f = BufReader::new(f);
        let _ = f.seek(SeekFrom::Start(start));
        GsmSource {
            f,
            wav49,
            data_start: start,
            data_len: len,
            total,
            pos: 0,
            consumed: 0,
            dec: gsm::Decoder::new(),
            pcm: [0; gsm::WAV49_BLOCK_SAMPLES],
            out: Vec::new(),
            skip: 0,
            info: AudioInfo {
                codec: "GSM 6.10".into(),
                sample_rate: rate,
                channels: 1,
                duration_secs: Some(samples as f64 / f64::from(rate)),
            },
        }
    }

    /// Decodes the next block or frame into `pcm`; returns the sample count.
    fn decode_unit(&mut self) -> Result<usize, AudioError> {
        let (unit_bytes, _) = unit(self.wav49);
        let mut b = [0u8; gsm::WAV49_BLOCK_BYTES];
        let b = &mut b[..unit_bytes as usize];
        let n = read_full(&mut self.f, b).map_err(io_err)?;
        if n < b.len() {
            return Err(AudioError::Damaged("incomplete GSM frame".into()));
        }
        let mut frame = [0i16; gsm::FRAME_SAMPLES];
        if self.wav49 {
            let block: &[u8; gsm::WAV49_BLOCK_BYTES] = (&*b).try_into().expect("65 bytes");
            for (i, fr) in gsm::unpack_wav49(block).iter().enumerate() {
                self.dec.decode(fr, &mut frame);
                self.pcm[i * 160..i * 160 + 160].copy_from_slice(&frame);
            }
            Ok(gsm::WAV49_BLOCK_SAMPLES)
        } else {
            let raw: &[u8; gsm::FRAME_BYTES] = (&*b).try_into().expect("33 bytes");
            let fr = gsm::unpack_raw(raw)
                .ok_or_else(|| AudioError::Damaged("bad GSM frame signature".into()))?;
            self.dec.decode(&fr, &mut frame);
            self.pcm[..160].copy_from_slice(&frame);
            Ok(gsm::FRAME_SAMPLES)
        }
    }
}

/// Bytes and samples of one read unit.
fn unit(wav49: bool) -> (u64, u64) {
    if wav49 {
        (
            gsm::WAV49_BLOCK_BYTES as u64,
            gsm::WAV49_BLOCK_SAMPLES as u64,
        )
    } else {
        (gsm::FRAME_BYTES as u64, gsm::FRAME_SAMPLES as u64)
    }
}

impl AudioSource for GsmSource {
    fn info(&self) -> AudioInfo {
        self.info.clone()
    }

    fn next_block(&mut self) -> Result<Option<&[f32]>, AudioError> {
        loop {
            let (unit_bytes, _) = unit(self.wav49);
            // Up to the declared end of the data (a WAV may hold chunks after it).
            let left = self.data_len.saturating_sub(self.consumed);
            if left == 0 {
                return Ok(None);
            }
            if left < unit_bytes {
                return Err(AudioError::Damaged("incomplete GSM frame".into()));
            }
            let n = self.decode_unit()? as u64;
            self.consumed += unit_bytes;
            let keep = match self.total {
                Some(t) => n.min(t.saturating_sub(self.pos)),
                None => n,
            };
            self.pos += n;
            if keep == 0 {
                return Ok(None);
            }
            let skip = self.skip.min(keep as usize);
            self.skip -= skip;
            self.out.clear();
            self.out.extend(
                self.pcm[skip..keep as usize]
                    .iter()
                    .map(|&s| f32::from(s) / 32768.0),
            );
            if !self.out.is_empty() {
                return Ok(Some(&self.out));
            }
        }
    }

    fn seek(&mut self, secs: f64) -> Result<(), AudioError> {
        let secs = clamp_secs(secs, self.info.duration_secs);
        let target = (secs * f64::from(self.info.sample_rate)) as u64;
        let (unit_bytes, unit_samples) = unit(self.wav49);
        let idx = target / unit_samples;
        self.f
            .seek(SeekFrom::Start(self.data_start + idx * unit_bytes))
            .map_err(io_err)?;
        // The decoder state is not stored per frame; it settles within a few frames.
        self.dec = gsm::Decoder::new();
        self.pos = idx * unit_samples;
        self.consumed = idx * unit_bytes;
        self.skip = (target - self.pos) as usize;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(59.9), "0:59");
        assert_eq!(clock(61.0), "1:01");
        assert_eq!(clock(3600.0), "60:00");
        assert_eq!(clock(f64::NAN), "0:00");
    }

    #[test]
    fn remap_channels() {
        assert_eq!(remap(&[1.0, 2.0], 1, 2), vec![1.0, 1.0, 2.0, 2.0]);
        assert_eq!(
            remap(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 3, 2),
            vec![1.0, 2.0, 4.0, 5.0]
        );
    }

    #[test]
    fn wav_tag_labels() {
        assert_eq!(wav_tag_label(0x0045), "G.726 (WAV format tag 0x0045)");
        assert_eq!(wav_tag_label(0x1234), "WAV format tag 0x1234");
    }
}
