//! A baseline JPEG encoder (ITU-T T.81), AniCraft's own: sequential DCT, Huffman coding, 4:2:0
//! chroma subsampling, quality-scaled Annex K tables. It writes the preview image of a `.moho`
//! export package (Moho regenerates it on its own saves, so the encoder stays small and exact
//! rather than fast).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Luminance quantisation table, zigzag order (T.81 Annex K, table K.1).
const LUMA_Q: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
/// Chrominance quantisation table, zigzag order (Annex K, table K.2).
const CHROMA_Q: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];
/// Zigzag order: index = zigzag position, value = the sample's natural position (T.81 A.6).
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];
/// Annex K Huffman table, luminance DC: 16 counts then the values.
const LUMA_DC: [u8; 28] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
/// Annex K Huffman table, luminance AC.
const LUMA_AC: [u8; 178] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA];
/// Annex K Huffman table, chrominance DC.
const CHROMA_DC: [u8; 28] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
/// Annex K Huffman table, chrominance AC.
const CHROMA_AC: [u8; 178] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77, 0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xA1, 0xB1, 0xC1, 0x09, 0x23, 0x33, 0x52, 0xF0, 0x15, 0x62, 0x72, 0xD1, 0x0A, 0x16, 0x24, 0x34, 0xE1, 0x25, 0xF1, 0x17, 0x18, 0x19, 0x1A, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA];

/// A Huffman table with its code words derived from the Annex K counts (T.81 Annex C):
/// codes assigned in order, one longer each time a count runs out.
struct Huffman {
    codes: [u16; 256],
    lengths: [u8; 256],
}

impl Huffman {
    fn from_counts(spec: &[u8]) -> Huffman {
        let mut h = Huffman { codes: [0; 256], lengths: [0; 256] };
        let mut code = 0u16;
        let mut k = 16usize; // spec[0..16] are the counts, the values follow
        for (len, &count) in spec.iter().enumerate().take(16) {
            for _ in 0..count {
                let value = spec[k] as usize;
                h.codes[value] = code;
                h.lengths[value] = (len + 1) as u8;
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        h
    }
}

/// MSB-first bit writer with JPEG's 0xFF byte stuffing (T.81 §B.1.1.5).
struct BitWriter {
    out: Vec<u8>,
    bits: u32,
    n: u8,
}

impl BitWriter {
    fn write(&mut self, code: u16, len: u8) {
        let mask = if len >= 16 { u16::MAX } else { (1u16 << len) - 1 };
        self.bits = (self.bits << len) | u32::from(code & mask);
        self.n += len;
        while self.n >= 8 {
            self.emit((self.bits >> (self.n - 8)) as u8);
            self.n -= 8;
        }
        self.bits &= (1 << self.n) - 1;
    }
    /// One bit's worth of a magnitude's value, MSB first.
    fn write_value(&mut self, value: i32, bits: u8) {
        if bits == 0 {
            return;
        }
        let mut shift = bits;
        while shift > 0 {
            shift -= 1;
            let bit = ((value >> shift) & 1) as u16;
            self.write(bit, 1);
        }
    }
    fn emit(&mut self, byte: u8) {
        self.out.push(byte);
        if byte == 0xFF {
            self.out.push(0); // byte stuffing
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.bits <<= pad;
            self.bits |= (1 << pad) - 1; // pad with 1s
            self.emit((self.bits & 0xFF) as u8);
        }
        self.out
    }
}

/// Quality 1..=100 into the table scale libjpeg made standard (T.81 leaves scaling open).
fn scaled_table(base: &[u8; 64], quality: u8) -> [u16; 64] {
    let q = quality.clamp(1, 100) as i32;
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    let mut out = [0u16; 64];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = ((base[i] as i32 * scale + 50) / 100).clamp(1, 255) as u16;
    }
    out
}

/// Forward DCT of one 8×8 block: separable float DCT-II, quantised with a natural-order
/// quant table, coefficients left in natural (row, column) order (T.81 A.3.3).
fn fdct(block: &[f32; 64], quant_natural: &[u16; 64]) -> [i16; 64] {
    let mut tmp = [0f32; 64];
    // Rows
    for y in 0..8 {
        for u in 0..8 {
            let mut sum = 0f32;
            for x in 0..8 {
                sum += block[y * 8 + x] * ((std::f32::consts::PI / 8.0) * (x as f32 + 0.5) * u as f32).cos();
            }
            let cu = if u == 0 { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
            tmp[y * 8 + u] = sum * cu * 0.5;
        }
    }
    // Columns
    let mut out = [0i16; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut sum = 0f32;
            for y in 0..8 {
                sum += tmp[y * 8 + u] * ((std::f32::consts::PI / 8.0) * (y as f32 + 0.5) * v as f32).cos();
            }
            let cv = if v == 0 { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
            let q = sum * cv * 0.5 / f32::from(quant_natural[v * 8 + u]);
            out[v * 8 + u] = q.round() as i16;
        }
    }
    out
}

/// A zigzag-order quant table (as the Annex K tables are written) in natural order.
fn natural_order(zigzag_quant: &[u16; 64]) -> [u16; 64] {
    let mut out = [0u16; 64];
    for (k, &q) in zigzag_quant.iter().enumerate() {
        out[ZIGZAG[k]] = q;
    }
    out
}

/// How many bits a difference magnitude needs (T.81 F.1.2.1): category = bits of |value|.
fn magnitude(value: i32) -> u8 {
    let v = value.unsigned_abs();
    if v == 0 {
        0
    } else {
        (32 - v.leading_zeros()) as u8
    }
}

/// Encodes straight-alpha RGBA (row-major) as a baseline JPEG. Quality 1..=100.
pub fn encode(rgba: &[u8], width: u32, height: u32, quality: u8) -> Vec<u8> {
    let (w, h) = (width.max(1) as usize, height.max(1) as usize);
    let (luma_dc, luma_ac, chroma_dc, chroma_ac) = (Huffman::from_counts(&LUMA_DC), Huffman::from_counts(&LUMA_AC), Huffman::from_counts(&CHROMA_DC), Huffman::from_counts(&CHROMA_AC));
    let (luma_q, chroma_q) = (scaled_table(&LUMA_Q, quality), scaled_table(&CHROMA_Q, quality)); // zigzag order, as DQT carries them
    let (luma_qn, chroma_qn) = (natural_order(&luma_q), natural_order(&chroma_q));
    // Y/Cb/Cr planes, full resolution for Y; chroma downsampled 2×2 by averaging.
    let mut y_plane = vec![0f32; w * h];
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut cb_plane = vec![0f32; cw * ch];
    let mut cr_plane = vec![0f32; cw * ch];
    let px = |x: usize, y: usize| -> [f32; 3] {
        let i = (y.min(h - 1) * w + x.min(w - 1)) * 4;
        [rgba[i] as f32, rgba[i + 1] as f32, rgba[i + 2] as f32]
    };
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = px(x, y);
            y_plane[y * w + x] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
            if x % 2 == 0 && y % 2 == 0 {
                let mut acc = [0f32; 3];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let c = px(x + dx, y + dy);
                    for k in 0..3 {
                        acc[k] += c[k] / 4.0;
                    }
                }
                cb_plane[(y / 2) * cw + x / 2] = -0.168736 * acc[0] - 0.331264 * acc[1] + 0.5 * acc[2];
                cr_plane[(y / 2) * cw + x / 2] = 0.5 * acc[0] - 0.418688 * acc[1] - 0.081312 * acc[2];
            }
        }
    }

    let mut out = Vec::new();
    out.extend_from_slice(&[0xFF, 0xD8]); // SOI
    // APP0 JFIF (a plain JFIF header keeps every decoder happy).
    out.extend_from_slice(&[0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]);
    // DQT: both tables, 8-bit precision.
    for (id, table) in [(0u8, &luma_q), (1u8, &chroma_q)] { // DQT in zigzag order
        out.extend_from_slice(&[0xFF, 0xDB, 0, 67, id]);
        for &q in table.iter() {
            out.push(q as u8);
        }
    }
    // SOF0: baseline, 8-bit, three components, 4:2:0 sampling.
    out.extend_from_slice(&[0xFF, 0xC0, 0, 17, 8]);
    out.extend_from_slice(&(h as u16).to_be_bytes());
    out.extend_from_slice(&(w as u16).to_be_bytes());
    out.push(3);
    out.extend_from_slice(&[1, 0x22, 0]); // Y: 2×2
    out.extend_from_slice(&[2, 0x11, 1]); // Cb: 1×1, quant 1
    out.extend_from_slice(&[3, 0x11, 1]); // Cr: 1×1, quant 1
    // DHT: four tables.
    for (class, id, spec) in [(0u8, 0u8, &LUMA_DC[..]), (1, 0, &LUMA_AC[..]), (0, 1, &CHROMA_DC[..]), (1, 1, &CHROMA_AC[..])] {
        // The segment length counts itself plus the class/id byte plus the table (16 counts
        // and the values).
        let len = 2 + 1 + spec.len();
        out.extend_from_slice(&[0xFF, 0xC4, (len >> 8) as u8, len as u8, (class << 4) | id]);
        out.extend_from_slice(spec);
    }
    // SOS
    out.extend_from_slice(&[0xFF, 0xDA, 0, 12, 3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
    let mut bits = BitWriter { out: Vec::new(), bits: 0, n: 0 };
    let mut y_dc = 0i32;
    let mut cb_dc = 0i32;
    let mut cr_dc = 0i32;
    let mut block = [0f32; 64];
    // One scan over interleaved MCUs (T.81 E.2.3): each MCU carries the 2×2 luma blocks that
    // cover its 16×16 samples, then one block of each chroma channel.
    let mut encode_block = |bits: &mut BitWriter, plane: &[f32], pw: usize, ph: usize, bx: usize, by: usize, quant: &[u16; 64], dc_table: &Huffman, ac_table: &Huffman, dc_pred: &mut i32| {
        for v in 0..8 {
            for u in 0..8 {
                let x = (bx * 8 + u).min(pw - 1);
                let y = (by * 8 + v).min(ph - 1);
                block[v * 8 + u] = plane[y * pw + x];
            }
        }
        let coeffs = fdct(&block, quant);
        let dc = coeffs[0] as i32;
        let diff = dc - *dc_pred;
        *dc_pred = dc;
        let cat = magnitude(diff);
        bits.write(dc_table.codes[cat as usize], dc_table.lengths[cat as usize]);
        if cat > 0 {
            // Negative values are written as the value one below a power of two
            // (T.81 F.1.2.1): the low `cat` bits of diff + (all ones above).
            let extra = if diff < 0 { diff + (1 << cat) - 1 } else { diff };
            bits.write_value(extra, cat);
        }
        let mut run = 0usize;
        for k in 1..64 {
            let c = coeffs[ZIGZAG[k]]; // the scan follows the zigzag order
            if c == 0 {
                run += 1;
                continue;
            }
            while run > 15 {
                bits.write(ac_table.codes[0xF0], ac_table.lengths[0xF0]); // ZRL
                run -= 16;
            }
            let symbol = (run << 4) | magnitude(c as i32) as usize;
            bits.write(ac_table.codes[symbol], ac_table.lengths[symbol]);
            let extra = if c < 0 { c as i32 + (1 << magnitude(c as i32)) - 1 } else { c as i32 };
            bits.write_value(extra, magnitude(c as i32));
            run = 0;
        }
        if run > 0 {
            bits.write(ac_table.codes[0x00], ac_table.lengths[0x00]); // EOB
        }
    };
    let mcus_x = w.div_ceil(16);
    let mcus_y = h.div_ceil(16);
    for my in 0..mcus_y {
        for mx in 0..mcus_x {
            for (dy, dx) in [(0usize, 0usize), (0, 1), (1, 0), (1, 1)] {
                encode_block(&mut bits, &y_plane, w, h, mx * 2 + dx, my * 2 + dy, &luma_qn, &luma_dc, &luma_ac, &mut y_dc);
            }
            encode_block(&mut bits, &cb_plane, cw, ch, mx, my, &chroma_qn, &chroma_dc, &chroma_ac, &mut cb_dc);
            encode_block(&mut bits, &cr_plane, cw, ch, mx, my, &chroma_qn, &chroma_dc, &chroma_ac, &mut cr_dc);
        }
    }
    out.extend_from_slice(&bits.finish());
    out.extend_from_slice(&[0xFF, 0xD9]); // EOI
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes with the workspace's JPEG decoder (zune-jpeg through doc's bitmap path) to check
    /// what we wrote is a real, correctly-sized JPEG with roughly our pixels.
    fn decode(bytes: &[u8], w: u32, h: u32) -> (u32, u32, Vec<u8>) {
        use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
        let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
        let mut dec = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
        let decoded = dec.decode().expect("test decode");
        let dims = dec.dimensions().expect("test dims");
        assert_eq!((dims.0 as u32, dims.1 as u32), (w, h), "dimensions");
        (dims.0 as u32, dims.1 as u32, decoded)
    }

    #[test]
    fn a_solid_colour_encodes_and_decodes_back_close() {
        let (w, h) = (32u32, 32u32);
        let mut px = vec![90u8, 140, 200, 255];
        px.resize((w * h * 4) as usize, 255);
        // to_rgba8-style fill: everything the same colour (alpha 255 everywhere).
        for i in 0..(w * h) as usize {
            px[i * 4..i * 4 + 3].copy_from_slice(&[90, 140, 200]);
            px[i * 4 + 3] = 255;
        }
        let jpeg = encode(&px, w, h, 90);
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xFF, 0xD9]);
        let (_, _, rgb) = decode(&jpeg, w, h);
        let mean: f64 = (0..(w * h) as usize).map(|i| (rgb[i * 3] as i64 - 90).abs() as f64).sum::<f64>() / f64::from(w * h);
        assert!(mean < 4.0, "mean red error {mean}");
    }

    #[test]
    fn odd_sizes_and_low_quality_still_decode() {
        let (w, h) = (17u32, 13u32);
        let px: Vec<u8> = (0..(w * h * 4) as usize).map(|i| if i % 4 == 3 { 255 } else { ((i * 7) % 256) as u8 }).collect();
        for q in [5u8, 50, 100] {
            let jpeg = encode(&px, w, h, q);
            decode(&jpeg, w, h);
        }
    }

    #[test]
    fn huffman_codes_match_the_annex_k_tables() {
        // Luminance DC: one code of length 2 (category 0 = 00), then five of length 3.
        let h = Huffman::from_counts(&LUMA_DC);
        assert_eq!(h.lengths[0], 2);
        assert_eq!(h.codes[0], 0b00);
        assert_eq!(h.lengths[1], 3);
        assert_eq!(h.codes[1], 0b010);
        assert_eq!(h.codes[2], 0b011);
        assert_eq!(h.codes[5], 0b110);
    }
}
