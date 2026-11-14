//! Decoding of the audio fixtures (see `fixtures/audio/make_fixtures.py` for how they were made).
//! Every fixture is a synthetic sweep: 200 Hz -> 3 kHz, amplitude 0.5.

use fxfm_core::audio::{self, AudioError, AudioInfo, AudioSource};
use std::f64::consts::PI;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

/// The sweep the fixtures were made from (channel 1 of stereo is phase-shifted by 1 rad).
fn sweep(rate: u32, n: usize, ch: usize) -> Vec<f32> {
    let t_total = n as f64 / f64::from(rate);
    let mut out = Vec::with_capacity(n * ch);
    for i in 0..n {
        let t = i as f64 / f64::from(rate);
        let ph = 2.0 * PI * (200.0 * t + (3000.0 - 200.0) * t * t / (2.0 * t_total));
        for c in 0..ch {
            out.push((0.5 * (ph + c as f64).sin()) as f32);
        }
    }
    out
}

/// Decodes everything; the error (if any) after the samples.
fn decode_all(src: &mut dyn AudioSource) -> (Vec<f32>, Option<AudioError>) {
    let mut out = Vec::new();
    loop {
        match src.next_block() {
            Ok(Some(b)) => out.extend_from_slice(b),
            Ok(None) => return (out, None),
            Err(e) => return (out, Some(e)),
        }
    }
}

fn decode(name: &str) -> (AudioInfo, Vec<f32>) {
    let mut src = audio::open(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let info = src.info();
    let (pcm, err) = decode_all(src.as_mut());
    assert_eq!(err, None, "{name}");
    (info, pcm)
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

fn rms(a: &[f32]) -> f32 {
    (a.iter().map(|x| x * x).sum::<f32>() / a.len().max(1) as f32).sqrt()
}

/// Normalised cross-correlation at lag 0.
fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    dot / (rms(a) * rms(b) * n as f32)
}

/// A lossless fixture: exact length and every sample within `tol` of the sweep.
fn check_lossless(name: &str, rate: u32, n: usize, ch: usize, codec: &str, tol: f32) {
    let (info, pcm) = decode(name);
    assert_eq!(info.sample_rate, rate, "{name}");
    assert_eq!(usize::from(info.channels), ch, "{name}");
    assert_eq!(info.codec, codec, "{name}");
    assert_eq!(pcm.len(), n * ch, "{name}");
    let want = sweep(rate, n, ch);
    let d = max_abs_diff(&pcm, &want);
    assert!(d <= tol, "{name}: max diff {d} > {tol}");
    let dur = info.duration_secs.expect("duration");
    assert!(
        (dur - n as f64 / f64::from(rate)).abs() < 1e-3,
        "{name}: {dur}"
    );
}

/// A lossy fixture: the frame count within `slack` of `n` and the sweep recognisable.
fn check_lossy(name: &str, rate: u32, n: usize, ch: usize, codec: &str, slack: usize) -> Vec<f32> {
    let (info, pcm) = decode(name);
    assert_eq!(info.sample_rate, rate, "{name}");
    assert_eq!(usize::from(info.channels), ch, "{name}");
    assert_eq!(info.codec, codec, "{name}");
    let frames = pcm.len() / ch;
    assert!(
        frames.abs_diff(n) <= slack,
        "{name}: {frames} frames, want {n} ± {slack}"
    );
    // The sweep's level (0.5 / sqrt 2) survives encoding.
    let level = rms(&pcm);
    assert!((0.25..0.45).contains(&level), "{name}: rms {level}");
    pcm
}

const HALF_LSB_16: f32 = 1.0 / 32768.0;

#[test]
fn wav_pcm_8_16_24_32_bit() {
    check_lossless("pcm_u8.wav", 8000, 2000, 1, "PCM 8-bit", 1.0 / 128.0);
    check_lossless("pcm_s16.wav", 8000, 2000, 1, "PCM 16-bit", HALF_LSB_16);
    check_lossless("pcm_s24.wav", 8000, 2000, 1, "PCM 24-bit", 1e-6);
    check_lossless("pcm_s32.wav", 8000, 2000, 1, "PCM 32-bit", 1e-6);
}

#[test]
fn wav_float() {
    check_lossless("float32.wav", 8000, 2000, 1, "PCM float 32-bit", 1e-7);
}

#[test]
fn wav_extensible_resolves_sub_format() {
    check_lossless(
        "extensible_s16_stereo.wav",
        8000,
        2000,
        2,
        "PCM 16-bit",
        HALF_LSB_16,
    );
}

#[test]
fn wav_alaw_and_mulaw() {
    // G.711 keeps ~12 bits: below 0.5 the largest step is 1/32, so the error stays under half
    // of it (plus the source's 16-bit rounding).
    let tol = 1.0 / 64.0 + 1.0 / 32768.0;
    check_lossless("alaw.wav", 8000, 8000, 1, "A-law", tol);
    check_lossless("mulaw.wav", 8000, 8000, 1, "mu-law", tol);
}

#[test]
fn wav_ms_and_ima_adpcm() {
    for (name, codec) in [("msadpcm.wav", "MS ADPCM"), ("ima_adpcm.wav", "IMA ADPCM")] {
        // Block padding may add up to one block of frames.
        let pcm = check_lossy(name, 8000, 8000, 1, codec, 1024);
        let c = correlation(&pcm, &sweep(8000, 8000, 1));
        assert!(c > 0.98, "{name}: correlation {c}");
    }
}

#[test]
fn wav_mp3_data_goes_to_the_mp3_decoder() {
    check_lossy("mp3_in_wav.wav", 8000, 8000, 1, "MP3", 1152 * 2);
}

#[test]
fn mp3_8k_mono_and_44k_stereo() {
    check_lossy("mp3_8k_mono.mp3", 8000, 8000, 1, "MP3", 1152 * 2);
    check_lossy("mp3_44k_stereo.mp3", 44100, 22050, 2, "MP3", 1152 * 2);
}

#[test]
fn flac() {
    check_lossless("flac_8k.flac", 8000, 8000, 1, "FLAC", HALF_LSB_16);
}

#[test]
fn ogg_vorbis() {
    check_lossy("vorbis.ogg", 44100, 22050, 2, "Vorbis", 1024);
}

#[test]
fn ogg_opus() {
    // Opus always decodes at 48 kHz.
    check_lossy("opus.opus", 48000, 24000, 1, "Opus", 960 * 2);
}

#[test]
fn m4a_aac() {
    check_lossy("aac.m4a", 44100, 22050, 2, "AAC", 2048 * 2);
}

#[test]
fn aiff() {
    check_lossless("pcm.aiff", 8000, 8000, 1, "PCM 16-bit", HALF_LSB_16);
}

#[test]
fn au_mulaw() {
    check_lossless(
        "mulaw.au",
        8000,
        8000,
        1,
        "mu-law",
        1.0 / 64.0 + 1.0 / 32768.0,
    );
}

fn read_s16(name: &str) -> Vec<i16> {
    let bytes = std::fs::read(fixture(name)).unwrap();
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect()
}

fn to_i16(pcm: &[f32]) -> Vec<i16> {
    pcm.iter().map(|&s| (s * 32768.0).round() as i16).collect()
}

/// `gsm.afconvert.s16` is macOS AudioToolbox's decode of `gsm_wav49.wav` (`afconvert -d LEI16`),
/// `gsm.ffmpeg.s16` ffmpeg's (`gsm_ms` decoder).
#[test]
fn gsm_wav49_matches_reference() {
    let (info, pcm) = decode("gsm_wav49.wav");
    assert_eq!(info.codec, "GSM 6.10");
    assert_eq!((info.sample_rate, info.channels), (8000, 1));
    assert_eq!(info.duration_secs, Some(0.96));
    let ours = to_i16(&pcm);
    assert_eq!(ours.len(), 7680);
    assert_eq!(
        ours,
        read_s16("gsm.afconvert.s16"),
        "differs from AudioToolbox"
    );
    // ffmpeg agrees on the speech-like first 24 frames. On the random-bit frames after them it
    // departs from ETSI 06.10 (e.g. it clamps an out-of-range LTP lag instead of reusing the
    // previous one).
    let ffmpeg = read_s16("gsm.ffmpeg.s16");
    assert_eq!(ours[..24 * 160], ffmpeg[..24 * 160], "differs from ffmpeg");
}

#[test]
fn raw_gsm_file() {
    // The same 48 frames as gsm_wav49.wav in 33-byte packing decode to the same samples.
    let (info, pcm) = decode("raw.gsm");
    assert_eq!(info.codec, "GSM 6.10");
    assert_eq!((info.sample_rate, info.channels), (8000, 1));
    assert_eq!(to_i16(&pcm), read_s16("gsm.afconvert.s16"));
}

fn temp_copy(name: &str, f: impl FnOnce(&mut Vec<u8>)) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = std::fs::read(fixture(name)).unwrap();
    f(&mut bytes);
    let p = dir.path().join(name);
    std::fs::write(&p, bytes).unwrap();
    (dir, p)
}

fn open_err(p: &std::path::Path) -> AudioError {
    match audio::open(p) {
        Ok(_) => panic!("{} opened", p.display()),
        Err(e) => e,
    }
}

#[test]
fn unsupported_tag_names_codec() {
    // WMA tag in an otherwise valid PCM header.
    let (_d, p) = temp_copy("pcm_s16.wav", |b| {
        b[20..22].copy_from_slice(&0x0161u16.to_le_bytes())
    });
    let e = open_err(&p);
    assert!(
        matches!(&e, AudioError::Unsupported(s) if s.contains("0x0161")),
        "{e:?}"
    );
    // G.726 is named.
    let e = open_err(&fixture("g726.wav"));
    assert!(
        matches!(&e, AudioError::Unsupported(s) if s.starts_with("G.726")),
        "{e:?}"
    );
    assert_eq!(
        e.to_string(),
        "unsupported codec G.726 (WAV format tag 0x0045)"
    );
}

#[test]
fn truncated_file_plays_then_reports_damage() {
    for (name, keep) in [
        ("pcm_s16.wav", 44 + 3000),
        ("gsm_wav49.wav", 60 + 65 * 10 + 20),
    ] {
        let (_d, p) = temp_copy(name, |b| b.truncate(keep));
        let mut src = audio::open(&p).unwrap();
        let (pcm, err) = decode_all(src.as_mut());
        assert!(
            matches!(err, Some(AudioError::Damaged(_))),
            "{name}: {err:?}"
        );
        assert!(!pcm.is_empty(), "{name}: nothing played");
    }
    // The GSM blocks before the cut are all there.
    let (_d, p) = temp_copy("gsm_wav49.wav", |b| b.truncate(60 + 65 * 10 + 20));
    let mut src = audio::open(&p).unwrap();
    let (pcm, _) = decode_all(src.as_mut());
    assert_eq!(to_i16(&pcm), read_s16("gsm.afconvert.s16")[..3200]);
}

#[test]
fn unfinished_header_plays_to_the_end() {
    // Data size left at 0 by a recorder that did not close the file.
    for name in ["gsm_wav49.wav", "alaw.wav", "pcm_s16.wav", "msadpcm.wav"] {
        let (_d, p) = temp_copy(name, |b| {
            let at = b.windows(4).position(|w| w == b"data").unwrap() + 4;
            b[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        });
        let mut src = audio::open(&p).unwrap();
        let (pcm, err) = decode_all(src.as_mut());
        assert_eq!(err, None, "{name}");
        assert!(pcm.len() >= 2000, "{name}: {}", pcm.len());
    }
}

#[test]
fn seek_lands_near_target() {
    for (name, rate, ch, tol_frames) in [
        ("pcm_s16.wav", 8000u32, 1usize, 0usize),
        ("flac_8k.flac", 8000, 1, 0),
        ("gsm_wav49.wav", 8000, 1, 0),
        ("mulaw.au", 8000, 1, 0),
        ("mp3_44k_stereo.mp3", 44100, 2, 1152),
    ] {
        let mut src = audio::open(&fixture(name)).unwrap();
        let (all, _) = decode_all(src.as_mut());
        let total = all.len() / ch;
        let target = 0.2;
        src.seek(target).unwrap();
        let (rest, err) = decode_all(src.as_mut());
        assert_eq!(err, None, "{name}");
        let left = rest.len() / ch;
        let want = total - (target * f64::from(rate)) as usize;
        assert!(
            left.abs_diff(want) <= tol_frames,
            "{name}: {left} frames left, want {want}"
        );
        if tol_frames == 0 && !name.starts_with("gsm") {
            assert_eq!(rest[..], all[all.len() - rest.len()..], "{name}");
        }
        // Back to the start plays everything again.
        src.seek(0.0).unwrap();
        let (again, err) = decode_all(src.as_mut());
        assert_eq!(err, None, "{name}");
        assert_eq!(again.len(), all.len(), "{name}");
    }
}

/// Offset of the `fmt ` chunk body in a fixture.
fn fmt_at(b: &[u8]) -> usize {
    b.windows(4).position(|w| w == b"fmt ").unwrap() + 8
}

/// Opens and decodes a broken file: it must fail or end cleanly, quickly, without panicking,
/// and no playable source may claim an absurd rate or channel count.
fn survives(name: &str, patch: impl FnOnce(&mut Vec<u8>)) -> Result<usize, AudioError> {
    let (_d, p) = temp_copy(name, patch);
    let start = std::time::Instant::now();
    let r = std::panic::catch_unwind(|| {
        let mut src = audio::open(&p)?;
        let info = src.info();
        assert!(
            (1..=audio::MAX_SAMPLE_RATE).contains(&info.sample_rate),
            "{info:?}"
        );
        assert!(
            (1..=audio::MAX_CHANNELS).contains(&info.channels),
            "{info:?}"
        );
        let (pcm, err) = decode_all(src.as_mut());
        match err {
            Some(e) if pcm.is_empty() => Err(e),
            _ => Ok(pcm.len()),
        }
    })
    .unwrap_or_else(|_| panic!("{name}: panicked"));
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "{name}: slow"
    );
    r
}

fn damaged(r: Result<usize, AudioError>, what: &str) {
    assert!(matches!(&r, Err(AudioError::Damaged(_))), "{what}: {r:?}");
}

#[test]
fn hostile_rates_and_channels_are_rejected() {
    for name in [
        "pcm_s16.wav",
        "alaw.wav",
        "gsm_wav49.wav",
        "msadpcm.wav",
        "ima_adpcm.wav",
    ] {
        for rate in [0u32, u32::MAX, 768_001] {
            let r = survives(name, |b| {
                let at = fmt_at(b);
                b[at + 4..at + 8].copy_from_slice(&rate.to_le_bytes());
            });
            damaged(r, &format!("{name} rate {rate}"));
        }
        let r = survives(name, |b| {
            let at = fmt_at(b);
            b[at + 2..at + 4].copy_from_slice(&0u16.to_le_bytes());
        });
        damaged(r, &format!("{name} channels 0"));
    }
}

#[test]
fn hostile_block_align_is_rejected() {
    for name in [
        "pcm_s16.wav",
        "msadpcm.wav",
        "ima_adpcm.wav",
        "gsm_wav49.wav",
    ] {
        let r = survives(name, |b| {
            let at = fmt_at(b);
            b[at + 12..at + 14].copy_from_slice(&0u16.to_le_bytes());
        });
        damaged(r, &format!("{name} block_align 0"));
    }
}

#[test]
fn data_size_past_eof_plays_what_is_there() {
    for name in [
        "pcm_s16.wav",
        "alaw.wav",
        "gsm_wav49.wav",
        "msadpcm.wav",
        "mp3_in_wav.wav",
    ] {
        let r = survives(name, |b| {
            let at = b.windows(4).position(|w| w == b"data").unwrap() + 4;
            b[at..at + 4].copy_from_slice(&0x7FFF_0000u32.to_le_bytes());
        });
        assert!(matches!(r, Ok(n) if n > 0), "{name}: {r:?}");
    }
}

#[test]
fn hostile_au_headers() {
    // Data offset past the end of the file.
    let r = survives("mulaw.au", |b| {
        b[4..8].copy_from_slice(&0x7FFF_FFFFu32.to_be_bytes())
    });
    assert!(matches!(r, Err(_) | Ok(0)), "{r:?}");
    for (at, v) in [(16, 0u32), (16, u32::MAX), (20, 0), (20, u32::MAX)] {
        let r = survives("mulaw.au", |b| {
            b[at..at + 4].copy_from_slice(&v.to_be_bytes())
        });
        damaged(r, &format!("au field {at} = {v}"));
    }
    // Data size far beyond the file.
    let r = survives("mulaw.au", |b| {
        b[8..12].copy_from_slice(&0x7FFF_0000u32.to_be_bytes())
    });
    assert!(
        r.is_ok() || matches!(r, Err(AudioError::Damaged(_))),
        "{r:?}"
    );
}
