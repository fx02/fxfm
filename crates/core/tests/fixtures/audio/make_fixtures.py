#!/usr/bin/env python3
"""Regenerates the audio test fixtures: python3 make_fixtures.py <out dir>.

Everything starts from a synthetic sweep (200 Hz -> 3 kHz, amplitude 0.5). The PCM WAVs and
the GSM files are written here; the rest was encoded on macOS with:

  afconvert -f WAVE -d alaw src8k.wav alaw.wav
  afconvert -f WAVE -d ulaw src8k.wav mulaw.wav
  afconvert -f AIFF -d BEI16 src8k.wav pcm.aiff
  afconvert -f NeXT -d ulaw src8k.wav mulaw.au
  afconvert -f m4af -d aac -b 96000 src44k_st.wav aac.m4a
  ffmpeg -i src8k.wav -c:a adpcm_ms msadpcm.wav
  ffmpeg -i src8k.wav -c:a adpcm_ima_wav ima_adpcm.wav
  ffmpeg -i src8k.wav -c:a libmp3lame -b:a 16k mp3_in_wav.wav      (WAV tag 0x0055)
  ffmpeg -i src8k.wav -c:a g726 -b:a 32k g726.wav                  (WAV tag 0x0045)
  ffmpeg -i src8k.wav -c:a flac flac_8k.flac
  ffmpeg -i src44k_st.wav -c:a vorbis -strict -2 vorbis.ogg
  ffmpeg -i src48k.wav -c:a libopus -b:a 16k opus.opus
  lame -b 16 src8k.wav mp3_8k_mono.mp3
  lame -b 128 --resample 44.1 src44k_st.wav mp3_44k_stereo.mp3

No GSM encoder exists on the Mac (afconvert only decodes GSM, ffmpeg lacks libgsm), so the GSM
bitstreams are built from synthetic frame parameters: frames 0..23 in speech-like ranges,
24..47 uniformly random bits. The references are decodes of gsm_wav49.wav:

  afconvert -f WAVE -d LEI16 gsm_wav49.wav x.wav   -> data chunk saved as gsm.afconvert.s16
  ffmpeg -i gsm_wav49.wav -f s16le gsm.ffmpeg.s16
"""
import math, random, struct, sys
def sweep(rate, n, ch=1, f0=200.0, f1=3000.0, amp=0.5):
    T = n / rate
    out = []
    for i in range(n):
        t = i / rate
        ph = 2*math.pi*(f0*t + (f1-f0)*t*t/(2*T))
        v = amp*math.sin(ph)
        out.append([v if c == 0 else amp*math.sin(ph*1.0 + 1.0) for c in range(ch)])
    return out
def wav(path, rate, frames, bits=16, tag=1, ext=False):
    ch = len(frames[0])
    data = bytearray()
    for fr in frames:
        for v in fr:
            if tag == 3:
                data += struct.pack('<f', v)
            elif bits == 8:
                data += struct.pack('<B', max(0, min(255, round(v*128)+128)))
            elif bits == 16:
                data += struct.pack('<h', max(-32768, min(32767, round(v*32768))))
            elif bits == 24:
                x = max(-(1<<23), min((1<<23)-1, round(v*(1<<23))))
                data += struct.pack('<i', x)[:3]
            elif bits == 32:
                x = max(-(1<<31), min((1<<31)-1, round(v*(1<<31))))
                data += struct.pack('<i', x)
    ba = ch*bits//8
    if ext:
        fmt = struct.pack('<HHIIHHHHI', 0xFFFE, ch, rate, rate*ba, ba, bits, 22, bits, 3 if ch==2 else 4)
        fmt += struct.pack('<H', tag) + b'\x00\x00\x00\x00\x10\x00\x80\x00\x00\xaa\x00\x38\x9b\x71'
    else:
        fmt = struct.pack('<HHIIHH', tag, ch, rate, rate*ba, ba, bits)
    body = b'WAVE' + b'fmt ' + struct.pack('<I', len(fmt)) + fmt + b'data' + struct.pack('<I', len(data)) + bytes(data)
    if len(data) % 2: body += b'\x00'
    open(path, 'wb').write(b'RIFF' + struct.pack('<I', len(body)) + body)
LAR_BITS = [6, 6, 5, 5, 4, 4, 3, 3]
def frame_fields(rng, plausible):
    f = []
    for b in LAR_BITS:
        mid = 1 << (b - 1)
        f.append((max(0, min((1 << b) - 1, mid + rng.randint(-(mid // 2), mid // 2))) if plausible else rng.getrandbits(b), b))
    for _ in range(4):
        f.append(((rng.randint(40, 120) if plausible else rng.getrandbits(7)), 7))
        f.append((rng.getrandbits(2), 2))
        f.append((rng.getrandbits(2), 2))
        f.append(((rng.randint(8, 30) if plausible else rng.getrandbits(6)), 6))
        for _ in range(13):
            f.append((rng.getrandbits(3), 3))
    assert sum(b for _, b in f) == 260
    return f
def pack_msb(bits_fields):
    acc, n, out = 0, 0, bytearray()
    for v, b in bits_fields:
        acc = (acc << b) | v; n += b
        while n >= 8:
            n -= 8; out.append((acc >> n) & 0xFF)
    assert n == 0
    return bytes(out)
def pack_lsb(bits_fields):
    acc, n, out = 0, 0, bytearray()
    for v, b in bits_fields:
        acc |= v << n; n += b
        while n >= 8:
            out.append(acc & 0xFF); acc >>= 8; n -= 8
    assert n == 0
    return bytes(out)


def gsm_files(out):
    rng = random.Random(0x6510)
    frames = [frame_fields(rng, i < 24) for i in range(48)]
    raw = b''.join(pack_msb([(0xD, 4)] + f) for f in frames)
    open(f'{out}/raw.gsm', 'wb').write(raw)
    data = b''.join(pack_lsb(frames[i] + frames[i + 1]) for i in range(0, 48, 2))
    fmt = struct.pack('<HHIIHHHH', 0x31, 1, 8000, 1625, 65, 0, 2, 320)
    body = (b'WAVE' + b'fmt ' + struct.pack('<I', len(fmt)) + fmt
            + b'fact' + struct.pack('<II', 4, 48 * 160)
            + b'data' + struct.pack('<I', len(data)) + data)
    open(f'{out}/gsm_wav49.wav', 'wb').write(b'RIFF' + struct.pack('<I', len(body)) + body)


if __name__ == '__main__':
    out = sys.argv[1]
    m8 = sweep(8000, 2000)        # 0.25 s, 8 kHz mono
    wav(f'{out}/pcm_u8.wav', 8000, m8, 8)
    wav(f'{out}/pcm_s16.wav', 8000, m8, 16)
    wav(f'{out}/pcm_s24.wav', 8000, m8, 24)
    wav(f'{out}/pcm_s32.wav', 8000, m8, 32)
    wav(f'{out}/float32.wav', 8000, m8, 32, tag=3)
    wav(f'{out}/extensible_s16_stereo.wav', 8000, sweep(8000, 2000, 2), 16, tag=1, ext=True)
    # Sources for the encoders (not committed).
    wav(f'{out}/src8k.wav', 8000, sweep(8000, 8000), 16)       # 1 s
    wav(f'{out}/src44k_st.wav', 44100, sweep(44100, 22050, 2), 16)  # 0.5 s stereo
    wav(f'{out}/src48k.wav', 48000, sweep(48000, 24000), 16)   # 0.5 s
    gsm_files(out)
