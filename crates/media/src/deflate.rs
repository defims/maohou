//! DEFLATE compression (RFC 1951), AniCraft's own writer for the zip archives the exports make
//! (a `.moho` package's members). Fixed Huffman codes only: one tree both sides know, so no
//! tree is stored, and LZ77 matches found with hash chains over a 32 K window. Lengths that a
//! fixed code can't reach (a match of 258 whose length code needs the tree) still encode: the
//! fixed length codes cover 3..=258 exactly, so every match encodes; only the match search
//! stays simple.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Window size (RFC 1951 §3.2.5).
const WINDOW: usize = 32_768;
/// The longest match a length code can name.
const MAX_MATCH: usize = 258;
const MIN_MATCH: usize = 3;
/// Longest chain followed per position: quality against speed.
const MAX_CHAIN: usize = 64;
/// Stop searching when a match this long is found.
const GOOD_MATCH: usize = 96;
/// How far a worse match may be from the chain's head before the search gives up.
const TOO_FAR: usize = 16_384;

struct BitWriter {
    out: Vec<u8>,
    bits: u32,
    n: u8,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { out: Vec::new(), bits: 0, n: 0 }
    }
    /// LSB-first, as RFC 1951 §3.1.1 packs codes.
    fn write(&mut self, value: u32, len: u8) {
        self.bits |= value << self.n;
        self.n += len;
        while self.n >= 8 {
            self.out.push((self.bits & 0xFF) as u8);
            self.bits >>= 8;
            self.n -= 8;
        }
    }
    /// Huffman codes go out most-significant bit first (§3.1.4) — write a code reversed.
    fn code(&mut self, code: u32, len: u8) {
        let mut rev = 0u32;
        for i in 0..len {
            rev |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.write(rev, len);
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push((self.bits & 0xFF) as u8);
        }
        self.out
    }
}

/// Fixed literal/length code lengths (RFC 1951 §3.2.6).
fn fixed_lit_len(symbol: u16) -> u8 {
    match symbol {
        0..=143 => 8,
        144..=255 => 9,
        256..=279 => 7,
        _ => 8,
    }
}
/// Fixed literal/length codes, as the RFC's table gives them.
fn fixed_lit_code(symbol: u16) -> u32 {
    match symbol {
        0..=143 => 0x30 + u32::from(symbol),
        144..=255 => 0x190 + u32::from(symbol) - 144,
        256..=279 => u32::from(symbol) - 256,
        _ => 0xC0 + u32::from(symbol) - 280,
    }
}
/// Distance base values for codes 0–29 (RFC 1951 §3.2.6).
const DIST_BASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
/// Distance extra-bit counts for codes 0–29.
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// The distance code of a distance, its extra bits and the extra value.
fn dist_code(dist: u16) -> (u16, u8, u16) {
    let mut code = 29usize;
    while DIST_BASE[code] > dist {
        code -= 1;
    }
    (code as u16, DIST_EXTRA[code], dist - DIST_BASE[code])
}
/// The length code of a match length 3..=258, and its extra bits.
fn length_code(len: u16) -> (u16, u8, u8) {
    const BASES: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
    const EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
    let mut code = 28usize;
    while code > 0 && len < BASES[code] {
        code -= 1;
    }
    (code as u16 + 257, (len - BASES[code]) as u8, EXTRA[code])
}

/// Compress `data` as a single fixed-Huffman DEFLATE stream.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter::new();
    // BFINAL=1 (one block), BTYPE=01 (fixed Huffman).
    w.write(1, 1);
    w.write(1, 2);
    // Hash chains: head[symbol] → most recent position with that 3-byte key, prev[pos % WINDOW]
    // → the position before it.
    let mut head = vec![u32::MAX; 1 << 15];
    let mut prev = vec![u32::MAX; WINDOW];
    let hash = |d: &[u8]| (u32::from(d[0]) << 10 ^ u32::from(d[1]) << 5 ^ u32::from(d[2])) & 0x7FFF;
    let mut i = 0usize;
    while i < data.len() {
        if i + MIN_MATCH <= data.len() {
            let key = hash(&data[i..]);
            let mut candidate = head[key as usize];
            let mut best_len = 0usize;
            let mut best_dist = 0u16;
            let max_len = MAX_MATCH.min(data.len() - i);
            let mut chain = MAX_CHAIN;
            while candidate != u32::MAX && chain > 0 {
                let pos = candidate as usize;
                if i.saturating_sub(pos) > WINDOW || i - pos > TOO_FAR {
                    break;
                }
                if best_len > 0 && data.get(i + best_len).copied() != data.get(pos + best_len).copied() {
                    candidate = prev[pos % WINDOW];
                    chain -= 1;
                    continue;
                }
                let mut l = 0usize;
                while l < max_len && data[i + l] == data[pos + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = (i - pos) as u16;
                    if l >= GOOD_MATCH {
                        break;
                    }
                }
                candidate = prev[pos % WINDOW];
                chain -= 1;
            }
            if best_len >= MIN_MATCH {
                let (sym, extra, extra_bits) = length_code(best_len as u16);
                w.code(fixed_lit_code(sym), fixed_lit_len(sym));
                if extra_bits > 0 {
                    w.write(u32::from(extra), extra_bits);
                }
                let (dcode, dbits, dextra) = dist_code(best_dist);
                w.code(u32::from(dcode), 5);
                if dbits > 0 {
                    w.write(u32::from(dextra), dbits);
                }
                for p in i..i + best_len {
                    if p + MIN_MATCH <= data.len() {
                        let key = hash(&data[p..]);
                        prev[p % WINDOW] = head[key as usize];
                        head[key as usize] = p as u32;
                    }
                }
                i += best_len;
                continue;
            }
            prev[i % WINDOW] = head[key as usize];
            head[key as usize] = i as u32;
        }
        let byte = data[i];
        let sym = u16::from(byte);
        w.code(fixed_lit_code(sym), fixed_lit_len(sym));
        i += 1;
    }
    w.code(fixed_lit_code(256), fixed_lit_len(256)); // end of block
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips through miniz_oxide's inflate (a dev-dependency; the real readers live in
    /// the importers, which are higher layers).
    fn inflate(data: &[u8]) -> Vec<u8> {
        miniz_oxide::inflate::decompress_to_vec(data).expect("test inflate")
    }

    #[test]
    fn round_trips_through_an_inflater() {
        // miniz_oxide is a dev-dependency only where the XFL crate brings it; this test lives
        // behind the same dependency in dev-dependencies.
        let cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"a".to_vec(),
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaa".to_vec(),
            b"abcabcabcabcabcabcabcabcabcabc".to_vec(),
            (0..100_000u32).map(|i| (i % 251) as u8).collect(),
            vec![7u8; 50_000],
        ];
        for data in &cases {
            let packed = deflate(data);
            assert_eq!(inflate(&packed), *data, "len {}", data.len());
        }
    }

    #[test]
    fn compresses_repeats() {
        let data = vec![b'x'; 10_000];
        let packed = deflate(&data);
        assert!(packed.len() < data.len() / 10, "{} vs 10000", packed.len());
    }
}
