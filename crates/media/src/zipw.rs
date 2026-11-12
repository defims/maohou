//! A zip archive writer (PKWARE's APPNOTE), AniCraft's own: the `.moho` export package
//! (`Project.mohoproj` + `preview.jpg`, as Moho writes them). Members go out with DEFLATE
//! ([`crate::deflate`]) or stored (an already-compressed preview JPEG), sizes and CRC known
//! up front, so no data descriptors; names are UTF-8 (flag bit 11), no ZIP64 (members under
//! 4 GB, as a document package is).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use crate::deflate;

/// DOS date/time (a zip member's stamp), little-endian fields packed per the APPNOTE. The
/// exports use a fixed stamp so the same document zips to the same bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DosTime {
    /// Year − 1980 in the high 7 bits, month, day (PACK appnote §4.4.2.3).
    pub date: u16,
    /// Hour, minute, 2-second second (§4.4.2.2).
    pub time: u16,
}

impl DosTime {
    /// 2026-01-01 00:00:00, the deterministic default stamp.
    pub fn fixed() -> Self {
        DosTime { date: (44 << 9) | (1 << 5) | 1, time: 0 }
    }
}

/// One member's central-directory row.
struct Central {
    name: String,
    crc: u32,
    packed: u32,
    size: u32,
    offset: u32,
    method: u16,
    time: DosTime,
}

/// A zip archive being built.
pub struct ZipWriter {
    out: Vec<u8>,
    central: Vec<Central>,
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// CRC-32/ISO-HDLC (the zip CRC: reflected, polynomial 0xEDB88320, initial and final all-ones).
pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

impl ZipWriter {
    pub fn new() -> Self {
        ZipWriter { out: Vec::new(), central: Vec::new() }
    }

    /// Adds a member, DEFLATE-compressed.
    pub fn write(&mut self, name: &str, data: &[u8]) {
        self.put(name, data, 8, crc32(data), deflate::deflate(data));
    }

    /// Adds a member stored as-is (already compressed: the preview JPEG).
    pub fn store(&mut self, name: &str, data: &[u8]) {
        self.put(name, data, 0, crc32(data), data.to_vec());
    }

    fn put(&mut self, name: &str, data: &[u8], method: u16, crc: u32, packed: Vec<u8>) {
        let offset = self.out.len() as u32;
        let time = DosTime::fixed();
        self.out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        self.out.extend_from_slice(&20u16.to_le_bytes()); // version needed: 2.0 (deflate)
        self.out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        self.out.extend_from_slice(&method.to_le_bytes());
        self.out.extend_from_slice(&time.time.to_le_bytes());
        self.out.extend_from_slice(&time.date.to_le_bytes());
        self.out.extend_from_slice(&crc.to_le_bytes());
        self.out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        self.out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes()); // no extra field
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(&packed);
        self.central.push(Central { name: name.to_string(), crc, packed: packed.len() as u32, size: data.len() as u32, offset, method, time });
    }

    /// Finishes the archive: the central directory and its end record.
    pub fn finish(mut self) -> Vec<u8> {
        let dir = self.out.len() as u32;
        for e in &self.central {
            self.out.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
            self.out.extend_from_slice(&20u16.to_le_bytes()); // made by: 2.0
            self.out.extend_from_slice(&20u16.to_le_bytes()); // needed: 2.0
            self.out.extend_from_slice(&0x0800u16.to_le_bytes());
            self.out.extend_from_slice(&e.method.to_le_bytes());
            self.out.extend_from_slice(&e.time.time.to_le_bytes());
            self.out.extend_from_slice(&e.time.date.to_le_bytes());
            self.out.extend_from_slice(&e.crc.to_le_bytes());
            self.out.extend_from_slice(&e.packed.to_le_bytes());
            self.out.extend_from_slice(&e.size.to_le_bytes());
            self.out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            self.out.extend_from_slice(&0u16.to_le_bytes()); // extra
            self.out.extend_from_slice(&0u16.to_le_bytes()); // comment
            self.out.extend_from_slice(&0u16.to_le_bytes()); // disk number
            self.out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            self.out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            self.out.extend_from_slice(&e.offset.to_le_bytes());
            self.out.extend_from_slice(e.name.as_bytes());
        }
        let dir_len = self.out.len() as u32 - dir;
        self.out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes()); // this disk
        self.out.extend_from_slice(&0u16.to_le_bytes()); // dir disk
        self.out.extend_from_slice(&(self.central.len() as u16).to_le_bytes());
        self.out.extend_from_slice(&(self.central.len() as u16).to_le_bytes());
        self.out.extend_from_slice(&dir_len.to_le_bytes());
        self.out.extend_from_slice(&dir.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes()); // no comment
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the named member straight out of an archive's local header (test-only: the real
    /// readers live in the importers, which are higher layers).
    fn member(archive: &[u8], name: &str) -> Option<Vec<u8>> {
        let mut i = 0usize;
        while i + 4 <= archive.len() {
            if &archive[i..i + 4] != b"PK\x03\x04" {
                i += 1;
                continue;
            }
            let method = u16::from_le_bytes([archive[i + 8], archive[i + 9]]);
            let packed_len = u32::from_le_bytes(archive[i + 18..i + 22].try_into().ok()?) as usize;
            let name_len = u16::from_le_bytes(archive[i + 26..i + 28].try_into().ok()?) as usize;
            let extra_len = u16::from_le_bytes(archive[i + 28..i + 30].try_into().ok()?) as usize;
            let at = i + 30 + name_len + extra_len;
            if archive.get(i + 30..i + 30 + name_len) != Some(name.as_bytes()) {
                i = at + packed_len;
                continue;
            }
            let packed = archive.get(at..at + packed_len)?;
            return match method {
                0 => Some(packed.to_vec()),
                _ => miniz_oxide::inflate::decompress_to_vec(packed).ok(),
            };
        }
        None
    }

    #[test]
    fn members_round_trip_through_an_inflater() {
        let mut z = ZipWriter::new();
        z.write("Project.mohoproj", br#"{"layers": []}"#);
        z.store("preview.jpg", &[0xFF, 0xD8, 0xFF, 0xD9]);
        let archive = z.finish();
        assert_eq!(member(&archive, "Project.mohoproj").as_deref(), Some(&br#"{"layers": []}"#[..]));
        assert_eq!(member(&archive, "preview.jpg").as_deref(), Some(&[0xFFu8, 0xD8, 0xFF, 0xD9][..]));
        assert_eq!(member(&archive, "nope"), None);
    }
}
