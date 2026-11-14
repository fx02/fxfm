//! Plain sample data after a header: Sun/NeXT `.au` (`.snd`, big-endian PCM, A-law, mu-law)
//! and A-law/mu-law WAV (symphonia refuses their common 16-byte `fmt` chunk).

use super::{clamp_secs, io_err, read_full, AudioError, AudioInfo, AudioSource, WavHeader};
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Enc {
    Mulaw,
    Alaw,
    S8,
    S16,
    S24,
    S32,
    F32,
    F64,
}

impl Enc {
    fn bytes(self) -> usize {
        match self {
            Enc::Mulaw | Enc::Alaw | Enc::S8 => 1,
            Enc::S16 => 2,
            Enc::S24 => 3,
            Enc::S32 | Enc::F32 => 4,
            Enc::F64 => 8,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Enc::Mulaw => "mu-law",
            Enc::Alaw => "A-law",
            Enc::S8 => "PCM 8-bit",
            Enc::S16 => "PCM 16-bit",
            Enc::S24 => "PCM 24-bit",
            Enc::S32 => "PCM 32-bit",
            Enc::F32 => "PCM float 32-bit",
            Enc::F64 => "PCM float 64-bit",
        }
    }

    fn sample(self, b: &[u8]) -> f32 {
        match self {
            Enc::Mulaw => f32::from(ulaw_to_linear(b[0])) / 32768.0,
            Enc::Alaw => f32::from(alaw_to_linear(b[0])) / 32768.0,
            Enc::S8 => f32::from(b[0] as i8) / 128.0,
            Enc::S16 => f32::from(i16::from_be_bytes([b[0], b[1]])) / 32768.0,
            Enc::S24 => (i32::from_be_bytes([b[0], b[1], b[2], 0]) >> 8) as f32 / 8_388_608.0,
            Enc::S32 => i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            Enc::F32 => f32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            Enc::F64 => f64::from_be_bytes(b[..8].try_into().expect("8 bytes")) as f32,
        }
    }
}

/// G.711 mu-law to 16-bit linear.
pub(crate) fn ulaw_to_linear(u: u8) -> i16 {
    let u = !u;
    let t = ((i32::from(u & 0x0F) << 3) + 0x84) << ((u & 0x70) >> 4);
    (if u & 0x80 != 0 { 0x84 - t } else { t - 0x84 }) as i16
}

/// G.711 A-law to 16-bit linear.
pub(crate) fn alaw_to_linear(a: u8) -> i16 {
    let a = a ^ 0x55;
    let mut t = i32::from(a & 0x0F) << 4;
    let seg = (a & 0x70) >> 4;
    match seg {
        0 => t += 8,
        1 => t += 0x108,
        _ => t = (t + 0x108) << (seg - 1),
    }
    (if a & 0x80 != 0 { t } else { -t }) as i16
}

struct PcmSource {
    f: BufReader<File>,
    enc: Enc,
    data_start: u64,
    /// Data bytes; None when the header leaves it open (read to the end).
    data_len: Option<u64>,
    consumed: u64,
    /// The last block ended early: the next call reports the damage.
    damaged: bool,
    buf: Vec<u8>,
    out: Vec<f32>,
    info: AudioInfo,
}

/// A-law (`alaw`) or mu-law WAV data.
pub(super) fn g711_wav(
    f: File,
    alaw: bool,
    h: &WavHeader,
) -> Result<Box<dyn AudioSource>, AudioError> {
    let enc = if alaw { Enc::Alaw } else { Enc::Mulaw };
    source(
        f,
        enc,
        h.data_start,
        Some(h.data_len),
        h.rate,
        u32::from(h.channels),
    )
}

/// A `.au` file.
pub(super) fn open_au(mut f: File) -> Result<Box<dyn AudioSource>, AudioError> {
    let mut h = [0u8; 24];
    if read_full(&mut f, &mut h).map_err(io_err)? < 24 {
        return Err(AudioError::Damaged("short AU header".into()));
    }
    let be = |i: usize| u32::from_be_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
    let (offset, size, code, rate, channels) = (be(4), be(8), be(12), be(16), be(20));
    let enc = match code {
        1 => Enc::Mulaw,
        2 => Enc::S8,
        3 => Enc::S16,
        4 => Enc::S24,
        5 => Enc::S32,
        6 => Enc::F32,
        7 => Enc::F64,
        27 => Enc::Alaw,
        23 => return Err(AudioError::Unsupported("G.721 (AU encoding 23)".into())),
        24 => return Err(AudioError::Unsupported("G.722 (AU encoding 24)".into())),
        25 | 26 => {
            return Err(AudioError::Unsupported(format!(
                "G.723 (AU encoding {code})"
            )))
        }
        c => return Err(AudioError::Unsupported(format!("AU encoding {c}"))),
    };
    if offset < 24 {
        return Err(AudioError::Damaged("bad AU header".into()));
    }
    let data_len = (size != u32::MAX).then_some(u64::from(size));
    source(f, enc, u64::from(offset), data_len, rate, channels)
}

fn source(
    mut f: File,
    enc: Enc,
    data_start: u64,
    data_len: Option<u64>,
    rate: u32,
    channels: u32,
) -> Result<Box<dyn AudioSource>, AudioError> {
    if rate == 0 || channels == 0 || channels > 64 {
        return Err(AudioError::Damaged("no sample rate or channels".into()));
    }
    let file_len = f.metadata().map_err(io_err)?.len();
    let playable = data_len.unwrap_or(file_len.saturating_sub(data_start));
    let frame = (enc.bytes() * channels as usize) as u64;
    f.seek(SeekFrom::Start(data_start)).map_err(io_err)?;
    Ok(Box::new(PcmSource {
        f: BufReader::new(f),
        enc,
        data_start,
        data_len,
        consumed: 0,
        damaged: false,
        buf: Vec::new(),
        out: Vec::new(),
        info: AudioInfo {
            codec: enc.name().into(),
            sample_rate: rate,
            channels: channels as u16,
            duration_secs: Some((playable / frame) as f64 / f64::from(rate)),
        },
    }))
}

/// Frames per block.
const BLOCK_FRAMES: usize = 2048;

impl PcmSource {
    fn frame_bytes(&self) -> usize {
        self.enc.bytes() * usize::from(self.info.channels)
    }
}

impl AudioSource for PcmSource {
    fn info(&self) -> AudioInfo {
        self.info.clone()
    }

    fn next_block(&mut self) -> Result<Option<&[f32]>, AudioError> {
        if self.damaged {
            return Err(AudioError::Damaged("unexpected end of file".into()));
        }
        let fb = self.frame_bytes();
        let mut want = (BLOCK_FRAMES * fb) as u64;
        if let Some(len) = self.data_len {
            want = want.min(len.saturating_sub(self.consumed));
        }
        if want == 0 {
            return Ok(None);
        }
        self.buf.resize(want as usize, 0);
        let n = read_full(&mut self.f, &mut self.buf).map_err(io_err)?;
        self.consumed += n as u64;
        let whole = n / fb * fb;
        let short = (n as u64) < want && self.data_len.is_some() || whole < n;
        if whole == 0 {
            return if short {
                Err(AudioError::Damaged("unexpected end of file".into()))
            } else {
                Ok(None)
            };
        }
        let bytes = self.enc.bytes();
        self.out.clear();
        self.out.extend(
            self.buf[..whole]
                .chunks_exact(bytes)
                .map(|b| self.enc.sample(b)),
        );
        // Play what is here; the next call reports the damage.
        self.damaged = short;
        Ok(Some(&self.out))
    }

    fn seek(&mut self, secs: f64) -> Result<(), AudioError> {
        let secs = clamp_secs(secs, self.info.duration_secs);
        let frame = (secs * f64::from(self.info.sample_rate)) as u64;
        self.consumed = frame * self.frame_bytes() as u64;
        self.damaged = false;
        self.f
            .seek(SeekFrom::Start(self.data_start + self.consumed))
            .map_err(io_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g711_reference_values() {
        // ITU-T G.711 decoder output (16-bit, as sox/ffmpeg produce it).
        assert_eq!(ulaw_to_linear(0xFF), 0);
        assert_eq!(ulaw_to_linear(0x7F), 0);
        assert_eq!(ulaw_to_linear(0x00), -32124);
        assert_eq!(ulaw_to_linear(0x80), 32124);
        assert_eq!(alaw_to_linear(0xD5), 8);
        assert_eq!(alaw_to_linear(0x55), -8);
        assert_eq!(alaw_to_linear(0xAA), 32256);
        assert_eq!(alaw_to_linear(0x2A), -32256);
    }
}
