//! MP4 output (VP9 video, Opus audio), AniCraft's own writer for ISO-BMFF (ISO/IEC 14496-12):
//! `ftyp`, one `mdat` holding every sample (its size patched at the end), then `moov` with a
//! video track (`vp09` sample entry + `vpcC` per the public *VP Codec ISO Media File Format
//! Binding*: profile 0, 8-bit, 4:2:0, BT.709 limited) and an audio track (`opus` sample entry +
//! `dOps` per the public *Encapsulation of Opus in ISO-BMFF*, whose encoder delay is trimmed by
//! a version-1 edit list). Key frames are listed in `stss`; audio packet durations come from
//! each packet's TOC byte (RFC 6716 §3.1).

use crate::feed::Feed;
use crate::vp9enc::{EncoderConfig, Vp9Encoder, rgba_to_yuv420};

pub use crate::MovieOptions as Options;

fn u32v(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn u16v(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn u8v(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}
fn version_flags(out: &mut Vec<u8>) {
    u8v(out, 0);
    out.extend_from_slice(&[0, 0, 0]);
}
/// A box: four-character code, 32-bit size (patched later for `mdat`), payload. Returns where
/// the size field sits.
fn box_start(out: &mut Vec<u8>, kind: &[u8; 4]) -> usize {
    out.extend_from_slice(kind);
    let at = out.len();
    u32v(out, 0);
    at
}
fn box_end(out: &mut [u8], size_at: usize) {
    let size = (out.len() - size_at - 4) as u32;
    out[size_at..size_at + 4].copy_from_slice(&size.to_be_bytes());
}

/// The ISO-BMFF identity matrix: slots a, b, u, c, d, v, x, y, w (§6.2.2: unity is 0x00010000
/// in a, d and c; w carries 0x40000000).
const IDENTITY_MATRIX: [u8; 36] = [
    0, 1, 0, 0, // a = 1.0
    0, 0, 0, 0, // b
    0, 0, 0, 0, // u
    0, 1, 0, 0, // c = 1.0
    0, 1, 0, 0, // d = 1.0
    0, 0, 0, 0, // v
    0, 0, 0, 0, // x
    0, 0, 0, 0, // y
    0x40, 0, 0, 0, // w
];

/// Which track a `trak` box describes.
enum Trak {
    Video,
    Audio,
}

impl Trak {
    /// Track timescale and total media duration (in that timescale).
    fn timing(&self, s: &Mp4) -> (u32, u32) {
        match self {
            Trak::Video => (s.video_timescale, s.video_samples.iter().map(|x| x.delta).sum()),
            Trak::Audio => (48_000, s.audio_samples.iter().map(|x| x.delta).sum()),
        }
    }
    fn handler(&self) -> &'static [u8; 4] {
        match self {
            Trak::Video => b"vide",
            Trak::Audio => b"soun",
        }
    }
}

/// One sample in the movie: its bytes' place in the file, its length and its decode delta.
struct Sample {
    offset: u32,
    size: u32,
    delta: u32,
    key: bool,
}

/// An MP4 file being written. Frames and sound go in ([`Mp4::write_frame`], [`Mp4::write_audio`]),
/// the finished bytes come out of [`Mp4::finish`].
pub struct Mp4 {
    opts: Options,
    out: Vec<u8>,
    /// Where `mdat`'s size field sits (patched at finish).
    mdat_size_at: usize,
    /// Where `mdat`'s payload starts in the file.
    /// Write position for the next sample (absolute, in the file).
    at: u32,
    /// The video track's timescale (frames per second × 1000; delta = 1000 exactly).
    video_timescale: u32,
    video: Vp9Encoder,
    audio: Option<Feed>,
    video_samples: Vec<Sample>,
    audio_samples: Vec<Sample>,
    frames_written: u64,
}

/// `ftyp` (32 bytes) + the `mdat` box header (8).
const HEADER_LEN: u32 = 40;

impl Mp4 {
    pub fn new(opts: Options) -> Self {
        let (w, h) = (opts.width.max(1), opts.height.max(1));
        let fps = if opts.fps > 0.0 { opts.fps } else { 24.0 };
        let cfg = EncoderConfig {
            width: w,
            height: h,
            quality: opts.quality.clamp(1, 100),
            full_range: false,
            keyframe_interval: opts.keyframe_interval,
            target_kbps: None,
            frame_rate: fps,
            loop_filter: true,
        };
        let mut this = Mp4 {
            opts: Options { width: w, height: h, fps, ..opts },
            out: Vec::new(),
            mdat_size_at: 0,
            at: HEADER_LEN,
            video_timescale: (fps * 1000.0).round() as u32,
            video: Vp9Encoder::new(cfg),
            audio: Some(Feed::new(2, opts.audio_bitrate, opts.audio_rate.max(1))),
            video_samples: Vec::new(),
            audio_samples: Vec::new(),
            frames_written: 0,
        };
        this.header();
        this
    }

    fn header(&mut self) {
        // ftyp: isom, minor 2, brands isom/iso2/mp41.
        self.out.extend_from_slice(b"\x00\x00\x00\x20ftypisom\x00\x02\x00isomiso2mp41");
        let size_at = box_start(&mut self.out, b"mdat");
        self.mdat_size_at = size_at;
    }

    fn frame_delta(&self) -> u32 {
        (1000.0 / self.opts.fps).round().max(1.0) as u32
    }

    /// Adds one RGBA frame (row-major, `width × height × 4`) as the next video sample. Forces a
    /// key frame when none has been written yet.
    pub fn write_frame(&mut self, rgba: &[u8]) {
        let (w, h) = (self.opts.width, self.opts.height);
        let (y, u, v) = rgba_to_yuv420(rgba, w, h, false);
        let coded = self.video.encode_frame(&y, &u, &v, self.frames_written == 0);
        self.video_samples.push(Sample { offset: self.at, size: coded.data.len() as u32, delta: self.frame_delta(), key: coded.key });
        self.out.extend_from_slice(&coded.data);
        self.at += coded.data.len() as u32;
        self.frames_written += 1;
    }

    /// Adds interleaved stereo PCM at the options' rate; its Opus packets become audio samples.
    pub fn write_audio(&mut self, pcm: &[f32]) {
        let Some(feed) = self.audio.as_mut() else { return };
        for packet in feed.write(pcm) {
            self.push_audio_sample(packet);
        }
    }

    fn push_audio_sample(&mut self, packet: Vec<u8>) {
        let delta = opus_packet_ms(&packet).max(1);
        self.audio_samples.push(Sample { offset: self.at, size: packet.len() as u32, delta, key: true });
        self.out.extend_from_slice(&packet);
        self.at += packet.len() as u32;
    }

    /// Closes the audio, patches `mdat`'s size, writes `moov`, and returns the file.
    pub fn finish(mut self) -> Vec<u8> {
        if let Some(feed) = self.audio.as_mut() {
            for packet in feed.finish() {
                self.push_audio_sample(packet);
            }
        }
        let mdat_size = self.at - HEADER_LEN;
        self.out[self.mdat_size_at..self.mdat_size_at + 4].copy_from_slice(&mdat_size.to_be_bytes());
        self.moov();
        self.out
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames_written
    }

    /// The whole `moov` box, describing both tracks from the sample tables.
    fn moov(&mut self) {
        let moov_at = box_start(&mut self.out, b"moov");
        let video_ms: u32 = self.video_samples.iter().map(|s| s.delta).sum();
        let audio_ms: u32 = self.audio_samples.iter().map(|s| s.delta).sum();
        let duration_ms = video_ms.max(audio_ms);

        let b = box_start(&mut self.out, b"mvhd");
        version_flags(&mut self.out);
        u32v(&mut self.out, 0); // creation
        u32v(&mut self.out, 0); // modification
        u32v(&mut self.out, 1000); // timescale: ms
        u32v(&mut self.out, duration_ms);
        u32v(&mut self.out, 0x0001_0000); // rate
        u16v(&mut self.out, 0x0100); // volume
        self.out.extend_from_slice(&[0; 10]); // reserved
        self.out.extend_from_slice(&IDENTITY_MATRIX);
        self.out.extend_from_slice(&[0u8; 24]); // pre_defined
        u32v(&mut self.out, 3); // next_track_ID
        box_end(&mut self.out, b);

        let pre_skip = self.audio.as_ref().map_or(0, |f| f.pre_skip());
        self.trak(1, duration_ms, pre_skip, &Trak::Video);
        self.trak(2, duration_ms, pre_skip, &Trak::Audio);
        box_end(&mut self.out, moov_at);
    }

    /// A `trak` box: header, (audio: the pre-skip edit list), media header, handler, `stbl`.
    fn trak(&mut self, id: u32, duration_ms: u32, pre_skip: u16, kind: &Trak) {
        let trak_at = box_start(&mut self.out, b"trak");
        let b = box_start(&mut self.out, b"tkhd");
        version_flags(&mut self.out);
        self.out.extend_from_slice(&[0, 0, 7]); // flags: enabled, in movie, in preview
        u32v(&mut self.out, 0); // creation
        u32v(&mut self.out, 0); // modification
        u32v(&mut self.out, id); // track_ID
        u32v(&mut self.out, 0); // reserved
        u32v(&mut self.out, duration_ms);
        u32v(&mut self.out, 0); // reserved
        u32v(&mut self.out, 0); // reserved
        u16v(&mut self.out, 0); // layer
        u16v(&mut self.out, 0); // alternate_group
        u16v(&mut self.out, if matches!(kind, Trak::Audio) { 0x0100 } else { 0 }); // volume
        u16v(&mut self.out, 0); // reserved
        self.out.extend_from_slice(&IDENTITY_MATRIX);
        let (w, h) = match kind {
            Trak::Video => ((f64::from(self.opts.width) * 65536.0).round() as u32, (f64::from(self.opts.height) * 65536.0).round() as u32),
            Trak::Audio => (0, 0),
        };
        u32v(&mut self.out, w);
        u32v(&mut self.out, h);
        box_end(&mut self.out, b);

        if matches!(kind, Trak::Audio) {
            // edts: skip the encoder delay (media time = pre-skip; duration 0 = natural end).
            let edts_at = box_start(&mut self.out, b"edts");
            let elst_at = box_start(&mut self.out, b"elst");
            u8v(&mut self.out, 1); // version 1: 64-bit times
            self.out.extend_from_slice(&[0, 0, 0]);
            u32v(&mut self.out, 1); // entry_count
            u64v(&mut self.out, 0); // segment_duration: 0 = to the media's natural end
            u64v(&mut self.out, u64::from(pre_skip)); // media_time
            u16v(&mut self.out, 1); // media_rate_integer
            u16v(&mut self.out, 0); // media_rate_fraction
            box_end(&mut self.out, elst_at);
            box_end(&mut self.out, edts_at);
        }

        let (timescale, media_duration) = kind.timing(self);
        let mut mdia = vec![];
        let mdia_at = box_start(&mut mdia, b"mdia");
        let b = box_start(&mut mdia, b"mdhd");
        version_flags(&mut mdia);
        u32v(&mut mdia, 0); // creation
        u32v(&mut mdia, 0); // modification
        u32v(&mut mdia, timescale);
        u32v(&mut mdia, media_duration);
        u16v(&mut mdia, 0x55C4); // language "und"
        u16v(&mut mdia, 0); // pre_defined
        box_end(&mut mdia, b);
        let b = box_start(&mut mdia, b"hdlr");
        version_flags(&mut mdia);
        u32v(&mut mdia, 0); // pre_defined
        mdia.extend_from_slice(kind.handler());
        mdia.extend_from_slice(&[0; 12]); // reserved
        mdia.extend_from_slice(if matches!(kind, Trak::Audio) { b"AniCraft Sound Handler\0" } else { b"AniCraft Video Handler\0" });
        box_end(&mut mdia, b);
        let mut minf = vec![];
        let minf_at = box_start(&mut minf, b"minf");
        let b = box_start(&mut minf, if matches!(kind, Trak::Audio) { b"smhd" } else { b"vmhd" });
        version_flags(&mut minf);
        if matches!(kind, Trak::Audio) {
            u16v(&mut minf, 0); // balance
            u16v(&mut minf, 0); // reserved
        } else {
            u16v(&mut minf, 0); // graphicsmode: copy
            u16v(&mut minf, 0); // opcolor
            u16v(&mut minf, 0);
        }
        box_end(&mut minf, b);
        let mut dinf = vec![];
        let dref_b = box_start(&mut dinf, b"dref");
        version_flags(&mut dinf);
        u32v(&mut dinf, 1); // entry_count
        dinf.extend_from_slice(b"url ");
        u32v(&mut dinf, 12); // size
        dinf.extend_from_slice(&[0, 0, 1]); // version 0, flags: data in the same file
        box_end(&mut dinf, dref_b);
        minf.extend_from_slice(&dinf);
        let mut stbl_buf = vec![];
        let stbl_at = box_start(&mut stbl_buf, b"stbl");
        match kind {
            Trak::Video => self.video_stbl(&mut stbl_buf),
            Trak::Audio => self.audio_stbl(&mut stbl_buf, pre_skip),
        }
        box_end(&mut stbl_buf, stbl_at);
        minf.extend_from_slice(&stbl_buf);
        box_end(&mut minf, minf_at);
        mdia.extend_from_slice(&minf);
        box_end(&mut mdia, mdia_at);
        self.out.extend_from_slice(&mdia);
        box_end(&mut self.out, trak_at);
    }

    /// The video track's `stbl` (a `vp09` entry with its `vpcC`, and the sample tables).
    fn video_stbl(&self, out: &mut Vec<u8>) {
        let w = u16::try_from(self.opts.width).unwrap_or(u16::MAX);
        let h = u16::try_from(self.opts.height).unwrap_or(u16::MAX);
        let b = box_start(out, b"stsd");
        version_flags(out);
        u32v(out, 1); // entry_count
        let entry_at = box_start(out, b"vp09");
        out.extend_from_slice(&[0u8; 6]); // reserved
        u16v(out, 1); // data_reference_index
        u16v(out, 0); // pre_defined
        u16v(out, 0); // reserved
        u32v(out, 0); // pre_defined
        u32v(out, 0); // pre_defined
        u32v(out, 0); // pre_defined
        u16v(out, w);
        u16v(out, h);
        u32v(out, 0x0048_0000); // horizresolution: 72 dpi
        u32v(out, 0x0048_0000); // vertresolution
        u32v(out, 0); // reserved
        u16v(out, 1); // frame_count
        out.extend_from_slice(&[0u8; 32]); // compressorname
        u16v(out, 0x0018); // depth
        out.extend_from_slice(&[0xFF, 0xFF]); // pre_defined = -1
        // vpcC v1 (VP Codec ISO Media File Format Binding): profile 0, level unspecified,
        // 8-bit 4:2:0 limited-range BT.709.
        let vpcc_at = box_start(out, b"vpcC");
        u8v(out, 1); // version
        out.extend_from_slice(&[0]); // flags
        u8v(out, 0); // profile
        u8v(out, 0); // level: unspecified
        u8v(out, (8 << 4) | (1 << 1)); // bitDepth 8 | chroma 4:2:0 | fullRange off
        u8v(out, 1); // colour primaries: BT.709
        u8v(out, 1); // transfer characteristics: BT.709
        u8v(out, 1); // matrix coefficients: BT.709
        box_end(out, vpcc_at);
        box_end(out, entry_at);
        box_end(out, b);
        let delta = self.video_samples.first().map_or(0, |s| s.delta);
        stts(out, &[(self.video_samples.len() as u32, delta)]);
        stss(out, &self.video_samples);
        stsc_one_chunk_per_sample(out);
        stsz(out, &self.video_samples);
        stco(out, &self.video_samples);
    }

    /// The audio track's `stbl` (an `opus` entry with `dOps`, and the sample tables).
    fn audio_stbl(&self, out: &mut Vec<u8>, pre_skip: u16) {
        let b = box_start(out, b"stsd");
        version_flags(out);
        u32v(out, 1); // entry_count
        let entry_at = box_start(out, b"opus");
        out.extend_from_slice(&[0u8; 6]); // reserved
        u16v(out, 1); // data_reference_index
        u32v(out, 0); // reserved
        u32v(out, 0); // reserved
        u16v(out, 2); // channelcount
        u16v(out, 16); // samplesize
        u16v(out, 0); // pre_defined
        u16v(out, 0); // reserved
        u32v(out, 48_000 << 16); // samplerate (16.16)
        // dOps (Encapsulation of Opus in ISO-BMFF).
        let dops_at = box_start(out, b"dOps");
        u8v(out, 0); // version
        u8v(out, 2); // output channel count
        u16v(out, pre_skip);
        u32v(out, 48_000); // input sample rate
        u16v(out, 0); // output gain
        u8v(out, 0); // channel mapping family
        box_end(out, dops_at);
        box_end(out, entry_at);
        box_end(out, b);
        // Audio packet lengths vary (SILK rate control), so every sample gets its stts row; the
        // run-length compressor folds the equal ones together again.
        stts(out, &self.audio_samples.iter().map(|s| (1, s.delta)).collect::<Vec<_>>());
        stsc_one_chunk_per_sample(out);
        stsz(out, &self.audio_samples);
        stco(out, &self.audio_samples);
    }
}

/// `stts`: run-length (count, delta) pairs compressed from (count, delta) rows.
fn stts(out: &mut Vec<u8>, entries: &[(u32, u32)]) {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &(count, delta) in entries {
        if count == 0 {
            continue;
        }
        match runs.last_mut() {
            Some(last) if last.1 == delta => last.0 += count,
            _ => runs.push((count, delta)),
        }
    }
    let b = box_start(out, b"stts");
    version_flags(out);
    u32v(out, runs.len() as u32);
    for (count, delta) in &runs {
        u32v(out, *count);
        u32v(out, *delta);
    }
    box_end(out, b);
}

/// `stss`: the 1-based sample numbers of the key frames.
fn stss(out: &mut Vec<u8>, samples: &[Sample]) {
    let b = box_start(out, b"stss");
    version_flags(out);
    let keys: Vec<u32> = samples.iter().enumerate().filter(|(_, s)| s.key).map(|(i, _)| i as u32 + 1).collect();
    u32v(out, keys.len() as u32);
    for k in &keys {
        u32v(out, *k);
    }
    box_end(out, b);
}

/// `stsc` with one row: every sample is its own chunk.
fn stsc_one_chunk_per_sample(out: &mut Vec<u8>) {
    let b = box_start(out, b"stsc");
    version_flags(out);
    u32v(out, 1); // entry_count
    u32v(out, 1); // first_chunk
    u32v(out, 1); // samples_per_chunk
    u32v(out, 1); // sample_description_index
    box_end(out, b);
}

/// `stsz`: every sample's size.
fn stsz(out: &mut Vec<u8>, samples: &[Sample]) {
    let b = box_start(out, b"stsz");
    version_flags(out);
    u32v(out, 0); // sample_size: 0 = per-sample table follows
    u32v(out, samples.len() as u32);
    for s in samples {
        u32v(out, s.size);
    }
    box_end(out, b);
}

/// `stco`: every sample's absolute offset in the file.
fn stco(out: &mut Vec<u8>, samples: &[Sample]) {
    let b = box_start(out, b"stco");
    version_flags(out);
    u32v(out, samples.len() as u32);
    for s in samples {
        u32v(out, s.offset);
    }
    box_end(out, b);
}

fn u64v(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// An Opus packet's duration in milliseconds, from its TOC byte (RFC 6716 §3.1): SILK runs in
/// 10/20/40/60 ms by configuration, hybrid in 10/20 ms, CELT in 2.5/5/10/20 ms.
fn opus_packet_ms(packet: &[u8]) -> u32 {
    let Some(&toc) = packet.first() else { return 20 };
    match toc >> 3 {
        c @ 0..=11 => match c % 4 {
            0 => 10,
            1 => 20,
            2 => 40,
            _ => 60,
        },
        12 | 14 => 10,
        13 | 15 => 20,
        c => match c & 3 {
            0 => 3, // 2.5 ms, rounded: durations are whole milliseconds here
            1 => 5,
            2 => 10,
            _ => 20,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(w: u32, h: u32, fps: f64) -> Options {
        Options { width: w, height: h, fps, quality: 40, keyframe_interval: 5, audio_bitrate: 64_000, ..Default::default() }
    }

    fn frame(w: u32, h: u32, shade: u8) -> Vec<u8> {
        let mut px = vec![shade, shade, shade, 255];
        px.resize((w * h * 4) as usize, 255);
        px[0] = shade;
        px[1] = shade;
        px[2] = shade;
        px
    }

    /// The payload length of the first box named `kind` (32-bit size form).
    fn box_len(bytes: &[u8], kind: &[u8; 4]) -> Option<u32> {
        let mut i = 0usize;
        while i + 8 <= bytes.len() {
            if &bytes[i..i + 4] == kind {
                return Some(u32::from_be_bytes(bytes[i + 4..i + 8].try_into().ok()?));
            }
            i += 1;
        }
        None
    }

    #[test]
    fn a_tiny_movie_has_both_tracks_and_consistent_sizes() {
        let mut m = Mp4::new(opts(16, 16, 24.0));
        for i in 0..12u8 {
            m.write_frame(&frame(16, 16, i * 10));
            m.write_audio(&vec![0.0f32; 960 * 2]);
        }
        let bytes = m.finish();
        assert_eq!(&bytes[..8], &[0, 0, 0, 0x20, b'f', b't', b'y', b'p']);
        for kind in [b"mdat", b"moov", b"vp09", b"vpcC", b"opus", b"dOps", b"stss", b"edts", b"elst"] {
            assert!(box_len(&bytes, kind).is_some(), "missing {kind:?}");
        }
        // mdat's patched size covers exactly its payload: moov begins right after it.
        let mdat_at = bytes.windows(4).position(|w| w == *b"mdat").expect("mdat");
        let size = u32::from_be_bytes(bytes[mdat_at + 4..mdat_at + 8].try_into().expect("size")) as usize;
        let moov_at = bytes.windows(4).position(|w| w == *b"moov").expect("moov");
        assert_eq!(mdat_at + 8 + size, moov_at);
        assert!(size > 0);
    }

    #[test]
    fn packet_durations_follow_the_toc() {
        assert_eq!(opus_packet_ms(&[0x00]), 10); // config 0: SILK NB 10 ms
        assert_eq!(opus_packet_ms(&[0x08]), 20); // config 1: SILK NB 20 ms
        assert_eq!(opus_packet_ms(&[0x10]), 40); // config 2: SILK NB 40 ms
        assert_eq!(opus_packet_ms(&[0x28]), 20); // config 5: SILK MB 20 ms
        assert_eq!(opus_packet_ms(&[0x50]), 40); // config 10: SILK WB 40 ms
        assert_eq!(opus_packet_ms(&[0x60]), 10); // config 12: hybrid SWB 10 ms
        assert_eq!(opus_packet_ms(&[0x78]), 20); // config 15: hybrid FB 20 ms
        assert_eq!(opus_packet_ms(&[0xF8]), 20); // config 31: CELT FB 20 ms
        assert_eq!(opus_packet_ms(&[]), 20);
    }
}
