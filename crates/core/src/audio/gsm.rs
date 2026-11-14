//! GSM 06.10 full-rate (RPE-LTP) decoder, ETSI EN 300 961 / GSM 06.10, in 16-bit fixed point
//! exactly as the standard specifies (bit-exact with the reference decoder).
//!
//! Two packings are understood:
//! - raw frames (`.gsm`, AIFC `agsm`): 33 bytes, a 0xD signature nibble then the 260 parameter
//!   bits, most significant bit first;
//! - WAV49 (WAV format tag 0x0031): 65-byte blocks holding two frames (520 bits), least
//!   significant bit first.
//!
//! Both put the parameters in the same order: LARc[1..8], then per sub-frame Nc, bc, Mc, xmaxc
//! and xMc[0..12].

/// Samples per frame (20 ms at 8 kHz).
pub const FRAME_SAMPLES: usize = 160;
/// A raw frame.
pub const FRAME_BYTES: usize = 33;
/// A WAV49 block (two frames).
pub const WAV49_BLOCK_BYTES: usize = 65;
/// Samples per WAV49 block.
pub const WAV49_BLOCK_SAMPLES: usize = 2 * FRAME_SAMPLES;

// ---------------------------------------------------------------- ETSI 06.10 tables (5.4)

/// Table 5.1 / 5.2: decoding of the coded log-area ratios (B, MIC and INVA per LAR).
const B: [i16; 8] = [0, 0, 2048, -2560, 94, -1792, -341, -1144];
const MIC: [i16; 8] = [-32, -32, -16, -16, -8, -8, -4, -4];
const INVA: [i16; 8] = [13107, 13107, 13107, 13107, 19223, 17476, 31454, 29708];
/// Table 5.3b: quantization levels of the LTP gain.
const QLB: [i16; 4] = [3277, 11469, 21299, 32767];
/// Table 5.6: normalized inverse mantissa used to compute xM.
const FAC: [i16; 8] = [18431, 20479, 22527, 24575, 26623, 28671, 30719, 32767];
/// Number of bits of LARc[1..8].
const LAR_BITS: [u32; 8] = [6, 6, 5, 5, 4, 4, 3, 3];

/// The coded parameters of one 20 ms frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frame {
    pub larc: [i16; 8],
    pub nc: [i16; 4],
    pub bc: [i16; 4],
    pub mc: [i16; 4],
    pub xmaxc: [i16; 4],
    pub xmc: [[i16; 13]; 4],
}

// ---------------------------------------------------------------- basic operators (5.1)

fn add(a: i16, b: i16) -> i16 {
    a.saturating_add(b)
}

fn sub(a: i16, b: i16) -> i16 {
    a.saturating_sub(b)
}

/// mult_r: (a * b + 16384) >> 15, with -32768 * -32768 saturated.
fn mult_r(a: i16, b: i16) -> i16 {
    if a == i16::MIN && b == i16::MIN {
        i16::MAX
    } else {
        ((i32::from(a) * i32::from(b) + 16384) >> 15) as i16
    }
}

/// Arithmetic shift right; a negative `n` shifts left.
fn asr(a: i16, n: i32) -> i16 {
    if n >= 16 {
        -i16::from(a < 0)
    } else if n <= -16 {
        0
    } else if n < 0 {
        ((i32::from(a)) << -n) as i16
    } else {
        a >> n
    }
}

/// Arithmetic shift left; a negative `n` shifts right.
fn asl(a: i16, n: i32) -> i16 {
    if n >= 16 {
        0
    } else if n <= -16 {
        -i16::from(a < 0)
    } else if n < 0 {
        asr(a, -n)
    } else {
        ((i32::from(a)) << n) as i16
    }
}

// ---------------------------------------------------------------- unpacking

/// Reads fields from a bit string in either order.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    lsb_first: bool,
}

impl Bits<'_> {
    fn take(&mut self, n: u32) -> i16 {
        let mut v = 0u32;
        for i in 0..n {
            let (byte, bit) = (self.data[self.pos / 8], self.pos % 8);
            let b = if self.lsb_first {
                (byte >> bit) & 1
            } else {
                (byte >> (7 - bit)) & 1
            };
            if self.lsb_first {
                v |= u32::from(b) << i;
            } else {
                v = (v << 1) | u32::from(b);
            }
            self.pos += 1;
        }
        v as i16
    }

    fn frame(&mut self) -> Frame {
        let mut f = Frame::default();
        for (l, bits) in f.larc.iter_mut().zip(LAR_BITS) {
            *l = self.take(bits);
        }
        for j in 0..4 {
            f.nc[j] = self.take(7);
            f.bc[j] = self.take(2);
            f.mc[j] = self.take(2);
            f.xmaxc[j] = self.take(6);
            for x in f.xmc[j].iter_mut() {
                *x = self.take(3);
            }
        }
        f
    }
}

/// Parameters of a raw 33-byte frame; None without the 0xD signature.
pub fn unpack_raw(frame: &[u8; FRAME_BYTES]) -> Option<Frame> {
    if frame[0] >> 4 != 0xD {
        return None;
    }
    let mut b = Bits {
        data: frame,
        pos: 4,
        lsb_first: false,
    };
    Some(b.frame())
}

/// The two frames of a 65-byte WAV49 block.
pub fn unpack_wav49(block: &[u8; WAV49_BLOCK_BYTES]) -> [Frame; 2] {
    let mut b = Bits {
        data: block,
        pos: 0,
        lsb_first: true,
    };
    let first = b.frame();
    [first, b.frame()]
}

// ---------------------------------------------------------------- decoder

/// Decoder state (ETSI 06.10 section 4.3); one per stream, frames in order.
#[derive(Clone)]
pub struct Decoder {
    /// Reconstructed short-term residual: dp[-120..-1] history, then the current sub-frame.
    dp0: [i16; 280],
    /// Previous valid LTP lag.
    nrp: i16,
    /// Decoded LARs of this and the previous frame.
    larpp: [[i16; 8]; 2],
    j: usize,
    /// Short-term synthesis filter memory.
    v: [i16; 9],
    /// De-emphasis memory.
    msr: i16,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder {
            dp0: [0; 280],
            nrp: 40,
            larpp: [[0; 8]; 2],
            j: 0,
            v: [0; 9],
            msr: 0,
        }
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes one frame into 160 16-bit samples (5.3).
    pub fn decode(&mut self, f: &Frame, out: &mut [i16; FRAME_SAMPLES]) {
        let mut wt = [0i16; FRAME_SAMPLES];
        for j in 0..4 {
            let erp = rpe_decoding(f.xmaxc[j], f.mc[j], &f.xmc[j]);
            self.long_term_synthesis(f.nc[j], f.bc[j], &erp);
            wt[j * 40..j * 40 + 40].copy_from_slice(&self.dp0[120..160]);
            // dp[-120..-1] <- dp[-80..39]: shift the history by one sub-frame.
            self.dp0.copy_within(40..160, 0);
        }
        self.short_term_synthesis(&f.larc, &wt, out);
        self.postprocessing(out);
    }

    /// 5.3.2: long term synthesis filtering into dp[0..39] (dp0[120..160]).
    fn long_term_synthesis(&mut self, ncr: i16, bcr: i16, erp: &[i16; 40]) {
        let nr = if (40..=120).contains(&ncr) {
            ncr
        } else {
            self.nrp
        };
        self.nrp = nr;
        let brp = QLB[(bcr & 3) as usize];
        for (k, &e) in erp.iter().enumerate() {
            let drpp = mult_r(brp, self.dp0[120 + k - nr as usize]);
            self.dp0[120 + k] = add(e, drpp);
        }
    }

    /// 5.3.3/5.3.4 and 5.2.8-5.2.10: LAR decoding, interpolation and lattice synthesis.
    fn short_term_synthesis(&mut self, larc: &[i16; 8], wt: &[i16; 160], s: &mut [i16; 160]) {
        let cur = self.j;
        self.j ^= 1;
        let prev = self.j;
        // Decoding of the coded log-area ratios.
        for i in 0..8 {
            let mut t = add(larc[i], MIC[i]) << 10;
            t = sub(t, B[i] << 1);
            t = mult_r(INVA[i], t);
            self.larpp[cur][i] = add(t, t);
        }
        let (p, c) = (self.larpp[prev], self.larpp[cur]);
        let mut larp = [0i16; 8];
        // Samples 0..12: 3/4 previous + 1/4 current.
        for i in 0..8 {
            larp[i] = add(p[i] >> 2, c[i] >> 2);
            larp[i] = add(larp[i], p[i] >> 1);
        }
        self.synthesis(&larp_to_rp(larp), &wt[0..13], &mut s[0..13]);
        // 13..26: halfway.
        for i in 0..8 {
            larp[i] = add(p[i] >> 1, c[i] >> 1);
        }
        self.synthesis(&larp_to_rp(larp), &wt[13..27], &mut s[13..27]);
        // 27..39: 1/4 previous + 3/4 current.
        for i in 0..8 {
            larp[i] = add(p[i] >> 2, c[i] >> 2);
            larp[i] = add(larp[i], c[i] >> 1);
        }
        self.synthesis(&larp_to_rp(larp), &wt[27..40], &mut s[27..40]);
        // 40..159: current.
        self.synthesis(&larp_to_rp(c), &wt[40..160], &mut s[40..160]);
    }

    /// 5.3.4: short term synthesis lattice filter.
    fn synthesis(&mut self, rrp: &[i16; 8], wt: &[i16], sr: &mut [i16]) {
        let v = &mut self.v;
        for (out, &w) in sr.iter_mut().zip(wt) {
            let mut sri = w;
            for i in (0..8).rev() {
                sri = sub(sri, mult_r(rrp[i], v[i]));
                v[i + 1] = add(v[i], mult_r(rrp[i], sri));
            }
            v[0] = sri;
            *out = sri;
        }
    }

    /// 5.3.5: de-emphasis, upscaling and truncation to 13 bits.
    fn postprocessing(&mut self, s: &mut [i16; 160]) {
        let mut msr = self.msr;
        for x in s.iter_mut() {
            let tmp = mult_r(msr, 28180);
            msr = add(*x, tmp);
            *x = (add(msr, msr) as u16 & 0xFFF8) as i16;
        }
        self.msr = msr;
    }
}

/// 5.2.9.2: LARp to reflection coefficients rp.
fn larp_to_rp(mut larp: [i16; 8]) -> [i16; 8] {
    for l in larp.iter_mut() {
        let conv = |t: i16| -> i16 {
            if t < 11059 {
                t << 1
            } else if t < 20070 {
                t + 11059
            } else {
                add(t >> 2, 26112)
            }
        };
        *l = if *l < 0 {
            let t = if *l == i16::MIN { i16::MAX } else { -*l };
            -conv(t)
        } else {
            conv(*l)
        };
    }
    larp
}

/// 5.3.1/5.2.16-5.2.18: APCM inverse quantization and RPE grid positioning.
fn rpe_decoding(xmaxc: i16, mc: i16, xmc: &[i16; 13]) -> [i16; 40] {
    // xmaxc -> exponent and mantissa.
    let mut exp: i16 = 0;
    if xmaxc > 15 {
        exp = (xmaxc >> 3) - 1;
    }
    let mut mant = xmaxc - (exp << 3);
    if mant == 0 {
        exp = -4;
        mant = 7;
    } else {
        while mant <= 7 {
            mant = (mant << 1) | 1;
            exp -= 1;
        }
        mant -= 8;
    }
    let temp1 = FAC[mant as usize];
    let temp2 = sub(6, exp);
    let temp3 = asl(1, i32::from(sub(temp2, 1)));
    let mut ep = [0i16; 40];
    let start = (mc & 3) as usize;
    for (i, &x) in xmc.iter().enumerate() {
        let temp = ((x << 1) - 7) << 12; // restore sign, 16-bit signed
        let temp = add(mult_r(temp1, temp), temp3);
        ep[start + 3 * i] = asr(temp, i32::from(temp2));
    }
    ep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_and_wav49_unpack_alike() {
        // One frame of parameters packed both ways.
        let mut f = Frame {
            larc: [63, 1, 31, 2, 15, 3, 7, 4],
            ..Frame::default()
        };
        for j in 0..4 {
            f.nc[j] = 40 + j as i16 * 20;
            f.bc[j] = j as i16;
            f.mc[j] = 3 - j as i16;
            f.xmaxc[j] = 10 * j as i16 + 3;
            for (i, x) in f.xmc[j].iter_mut().enumerate() {
                *x = ((i + j) % 8) as i16;
            }
        }
        let mut fields: Vec<(u32, u32)> = Vec::new();
        for (l, b) in f.larc.iter().zip(LAR_BITS) {
            fields.push((*l as u32, b));
        }
        for j in 0..4 {
            fields.extend([
                (f.nc[j] as u32, 7),
                (f.bc[j] as u32, 2),
                (f.mc[j] as u32, 2),
            ]);
            fields.push((f.xmaxc[j] as u32, 6));
            fields.extend(f.xmc[j].iter().map(|&x| (x as u32, 3)));
        }
        let mut raw = [0u8; FRAME_BYTES];
        let mut pos = 0;
        for (v, b) in [(0xD, 4)].into_iter().chain(fields.iter().copied()) {
            for i in (0..b).rev() {
                if (v >> i) & 1 == 1 {
                    raw[pos / 8] |= 0x80 >> (pos % 8);
                }
                pos += 1;
            }
        }
        assert_eq!(unpack_raw(&raw), Some(f.clone()));
        let mut block = [0u8; WAV49_BLOCK_BYTES];
        let mut pos = 0;
        for (v, b) in fields.iter().chain(fields.iter()) {
            for i in 0..*b {
                if (v >> i) & 1 == 1 {
                    block[pos / 8] |= 1 << (pos % 8);
                }
                pos += 1;
            }
        }
        assert_eq!(unpack_wav49(&block), [f.clone(), f]);
        raw[0] = 0x0F;
        assert_eq!(unpack_raw(&raw), None);
    }
}
