//! WebM output (VP9 video, Opus audio), AniCraft's own writer for the Matroska/WebM container:
//! an EBML header (IETF RFC 8794), a Segment with its 8-byte size patched at the end, Info,
//! Tracks, one Cluster per batch of frames, with the Duration patched in at [`Webm::finish`].
//! Video frames are SimpleBlocks (a key frame carries the key flag); inter frames are
//! BlockGroups whose ReferenceBlock names the previous frame's timestamp. Audio is A_OPUS with
//! the RFC 7845 `OpusHead` as CodecPrivate, `CodecDelay` = the encoder's pre-skip and
//! `SeekPreRoll` = 80 ms, one SimpleBlock per 20 ms packet.

use crate::feed::{Feed, OPUS_RATE, PACKET_SAMPLES};
use crate::vp9enc::{EncoderConfig, Vp9Encoder, rgba_to_yuv420};

pub use crate::MovieOptions as Options;

/// One block waiting for its cluster.
struct Block {
    ms: i64,
    track: u64,
    key: bool,
    data: Vec<u8>,
}

/// Element id as bytes (shortest form).
fn id_bytes(id: u32) -> Vec<u8> {
    let b = id.to_be_bytes();
    let skip = b.iter().position(|x| *x != 0).unwrap_or(3);
    b[skip..].to_vec()
}

/// An EBML variable-size integer for a data size, shortest form (RFC 8794 §5).
fn vint(n: u64) -> Vec<u8> {
    for len in 1..=8u32 {
        let max = (1u64 << (7 * len)) - 2;
        if n <= max {
            let marked = n | (1u64 << (7 * len));
            return marked.to_be_bytes()[8 - len as usize..].to_vec();
        }
    }
    let mut v = vec![0x01];
    v.extend_from_slice(&n.to_be_bytes()[1..]);
    v
}

/// An element: id, size, payload.
fn el(out: &mut Vec<u8>, id: u32, data: &[u8]) {
    out.extend(id_bytes(id));
    out.extend(vint(data.len() as u64));
    out.extend_from_slice(data);
}
fn el_uint(out: &mut Vec<u8>, id: u32, v: u64) {
    let b = v.to_be_bytes();
    let skip = b.iter().position(|x| *x != 0).unwrap_or(7);
    el(out, id, &b[skip..]);
}
fn el_float(out: &mut Vec<u8>, id: u32, v: f64) {
    el(out, id, &v.to_be_bytes());
}
fn el_str(out: &mut Vec<u8>, id: u32, s: &str) {
    el(out, id, s.as_bytes());
}

/// A block payload: track number, timestamp relative to the cluster, flags, frame.
fn block(track: u64, rel: i16, flags: u8, data: &[u8]) -> Vec<u8> {
    let mut b = vint(track);
    b.extend_from_slice(&rel.to_be_bytes());
    b.push(flags);
    b.extend_from_slice(data);
    b
}

/// A WebM file being written. Frames and sound go in ([`Webm::write_frame`],
/// [`Webm::write_audio`]), the finished bytes come out of [`Webm::finish`].
pub struct Webm {
    opts: Options,
    out: Vec<u8>,
    /// Where the Segment's 8-byte size sits (patched at finish).
    seg_size_at: usize,
    /// Where the Info Duration's 8-byte float sits (patched at finish).
    duration_at: Option<usize>,
    video: Vp9Encoder,
    audio: Option<Feed>,
    /// Blocks awaiting their cluster.
    blocks: Vec<Block>,
    /// The previous video frame's timestamp (ReferenceBlock of inter frames).
    prev_video_ms: Option<i64>,
    frames_written: u64,
    /// Total duration for Info, as the last block's end in ms.
    last_ms: i64,
}

impl Webm {
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
        let audio = Some(Feed::new(2, opts.audio_bitrate, opts.audio_rate.max(1)));
        let mut this = Webm {
            opts: Options { width: w, height: h, fps, ..opts },
            out: Vec::new(),
            seg_size_at: 0,
            duration_at: None,
            video: Vp9Encoder::new(cfg),
            audio,
            blocks: Vec::new(),
            prev_video_ms: None,
            frames_written: 0,
            last_ms: 0,
        };
        this.header();
        this
    }

    /// EBML header, Segment opening, Info and Tracks.
    fn header(&mut self) {
        let mut head = vec![];
        let mut eb = vec![];
        el_uint(&mut eb, 0x4286, 1); // EBMLVersion
        el_uint(&mut eb, 0x42F7, 1); // EBMLReadVersion
        el_uint(&mut eb, 0x42F2, 4); // EBMLMaxIDLength
        el_uint(&mut eb, 0x42F3, 8); // EBMLMaxSizeLength
        el_str(&mut eb, 0x4282, "webm"); // DocType
        el_uint(&mut eb, 0x4287, 4); // DocTypeVersion
        el_uint(&mut eb, 0x4285, 2); // DocTypeReadVersion
        el(&mut head, 0x1A45_DFA3, &eb); // EBML
        head.extend(id_bytes(0x1853_8067)); // Segment
        self.seg_size_at = head.len();
        head.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // patched at finish
        let mut info = vec![];
        el_uint(&mut info, 0x2A_D7B1, 1_000_000); // TimestampScale: 1 ms
        let duration_in_info = info.len();
        el_float(&mut info, 0x4489, 0.0); // Duration: patched at finish
        el_str(&mut info, 0x4D80, "AniCraft"); // MuxingApp
        el_str(&mut info, 0x5741, "AniCraft"); // WritingApp
        // Where the Duration float will sit in `out`: the Info id (4 bytes), its size marker,
        // then the offset inside Info.
        self.duration_at = Some(head.len() + 4 + vint(info.len() as u64).len() + duration_in_info);
        el(&mut head, 0x1549_A966, &info); // Info
        let mut tracks = vec![];
        let mut v = vec![];
        el_uint(&mut v, 0xD7, 1); // TrackNumber
        el_uint(&mut v, 0x73C5, 1); // TrackUID
        el_uint(&mut v, 0x83, 1); // TrackType: video
        el_uint(&mut v, 0x9C, 0); // FlagLacing
        el_str(&mut v, 0x86, "V_VP9"); // CodecID
        let ns_per_frame = (1_000_000_000.0 / self.opts.fps).round() as u64; // DefaultDuration
        el_uint(&mut v, 0x23E383, ns_per_frame);
        let mut video = vec![];
        el_uint(&mut video, 0xB0, self.opts.width as u64); // PixelWidth
        el_uint(&mut video, 0xBA, self.opts.height as u64); // PixelHeight
        el(&mut v, 0xE0, &video); // Video
        el(&mut tracks, 0xAE, &v); // TrackEntry
        if let Some(a) = &self.audio {
            let mut t = vec![];
            el_uint(&mut t, 0xD7, 2);
            el_uint(&mut t, 0x73C5, 2);
            el_uint(&mut t, 0x83, 2); // TrackType: audio
            el_uint(&mut t, 0x9C, 0);
            el_str(&mut t, 0x86, "A_OPUS");
            el(&mut t, 0x63A2, &a.opus_head()); // CodecPrivate
            el_uint(&mut t, 0x56AA, u64::from(a.pre_skip()) * 1_000_000_000 / u64::from(OPUS_RATE)); // CodecDelay
            el_uint(&mut t, 0x56BB, 80_000_000); // SeekPreRoll
            let mut au = vec![];
            el_float(&mut au, 0xB5, f64::from(OPUS_RATE)); // SamplingFrequency
            el_uint(&mut au, 0x9F, u64::from(a.channels())); // Channels
            el(&mut t, 0xE1, &au); // Audio
            el(&mut tracks, 0xAE, &t);
        }
        el(&mut head, 0x1654_AE6B, &tracks); // Tracks
        self.out.extend_from_slice(&head);
    }

    /// Frame's timestamp in ms.
    fn frame_ms(&self, frame: u64) -> i64 {
        ((frame as f64 * 1000.0 / self.opts.fps).round() as i128).clamp(0, i64::MAX as i128) as i64
    }

    /// Adds one RGBA frame (row-major, `width × height × 4`), encoding it as the next video
    /// block. Forces a key frame when none has been written yet.
    pub fn write_frame(&mut self, rgba: &[u8]) {
        let (w, h) = (self.opts.width, self.opts.height);
        let (y, u, v) = rgba_to_yuv420(rgba, w, h, false);
        let coded = self.video.encode_frame(&y, &u, &v, self.frames_written == 0);
        let ms = self.frame_ms(self.frames_written);
        self.blocks.push(Block { ms, track: 1, key: coded.key, data: coded.data });
        self.prev_video_ms = Some(ms);
        self.last_ms = self.last_ms.max(ms + (1000.0 / self.opts.fps).round() as i64);
        self.frames_written += 1;
        if self.blocks.len() >= 32 {
            self.flush_cluster();
        }
    }

    /// Adds interleaved stereo PCM at the options' rate; its Opus packets become audio blocks.
    pub fn write_audio(&mut self, pcm: &[f32]) {
        let Some(feed) = self.audio.as_mut() else { return };
        for packet in feed.write(pcm) {
            let ms = feed.emitted() as i64 * 20 - 20;
            self.blocks.push(Block { ms, track: 2, key: true, data: packet });
            self.last_ms = self.last_ms.max(ms + 20);
        }
    }

    /// Closes the pending audio and writes the last cluster, patches the Segment size and the
    /// Duration, and returns the file.
    pub fn finish(mut self) -> Vec<u8> {
        if let Some(feed) = self.audio.as_mut() {
            for packet in feed.finish() {
                let ms = feed.emitted() as i64 * 20 - 20;
                self.blocks.push(Block { ms, track: 2, key: true, data: packet });
                self.last_ms = self.last_ms.max(ms + 20);
            }
        }
        self.flush_cluster();
        let seg_len = (self.out.len() - self.seg_size_at - 8) as u64;
        let mut size = [0xFFu8; 8];
        size[0] = 0x01;
        size[1..].copy_from_slice(&seg_len.to_be_bytes()[1..]);
        self.out[self.seg_size_at..self.seg_size_at + 8].copy_from_slice(&size);
        if let Some(at) = self.duration_at {
            self.out[at..at + 8].copy_from_slice(&(self.last_ms as f64).to_be_bytes());
        }
        self.out
    }

    /// Writes the waiting blocks as one Cluster (sorted by timestamp, as Matroska wants).
    fn flush_cluster(&mut self) {
        if self.blocks.is_empty() {
            return;
        }
        self.blocks.sort_by_key(|b| (b.ms, b.track));
        let base = self.blocks[0].ms;
        let mut body = vec![];
        el_uint(&mut body, 0xE7, base.max(0) as u64); // Timestamp
        for b in &self.blocks {
            let rel = (b.ms - base).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
            if b.track == 1 && !b.key {
                // An inter frame: a BlockGroup naming the previous frame as its reference.
                let mut g = vec![];
                el(&mut g, 0xA1, &block(1, rel, 0, &b.data)); // Block
                let back = self.prev_video_ms.map_or(-1, |p| (p - b.ms).clamp(i16::MIN as i64, -1) as i16);
                el(&mut g, 0xFB, &back.to_be_bytes()); // ReferenceBlock
                el(&mut body, 0xA0, &g); // BlockGroup
            } else {
                el(&mut body, 0xA3, &block(b.track, rel, if b.key { 0x80 } else { 0 }, &b.data)); // SimpleBlock
            }
        }
        let mut cluster = vec![];
        el(&mut cluster, 0x1F43_B675, &body); // Cluster
        self.out.extend_from_slice(&cluster);
        self.blocks.clear();
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames_written
    }

    /// The packet size (interleaved stereo samples per frame), for callers that chunk their PCM.
    pub fn audio_chunk(&self) -> usize {
        PACKET_SAMPLES * self.audio.as_ref().map_or(2, |f| f.channels() as usize)
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

    #[test]
    fn header_ids_and_a_tiny_movie() {
        assert_eq!(vint(0), vec![0x80]);
        assert_eq!(vint(126), vec![0xFE]);
        assert_eq!(vint(127), vec![0x40, 0x7F]);
        assert_eq!(id_bytes(0xA3), vec![0xA3]);
        assert_eq!(id_bytes(0x1A45_DFA3), vec![0x1A, 0x45, 0xDF, 0xA3]);

        let mut w = Webm::new(opts(16, 16, 24.0));
        for i in 0..12u8 {
            let px = frame(16, 16, i * 10);
            w.write_frame(&px);
            // stereo 20 ms of quiet tone per frame
            let pcm = vec![0.0f32; PACKET_SAMPLES * 2];
            w.write_audio(&pcm);
        }
        let bytes = w.finish();
        assert_eq!(&bytes[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        assert!(bytes.len() > 1000);
        assert!(bytes.windows(5).any(|w| w == b"V_VP9"));
        assert!(bytes.windows(6).any(|w| w == b"A_OPUS"));
        // The patched Segment size (8-byte form: marker + 7 value bytes) names exactly what
        // follows the size field.
        let seg_at = bytes.windows(5).position(|w| w == [0x18, 0x53, 0x80, 0x67, 0x01]).expect("segment id");
        let size = u64::from_be_bytes([0, bytes[seg_at + 5], bytes[seg_at + 6], bytes[seg_at + 7], bytes[seg_at + 8], bytes[seg_at + 9], bytes[seg_at + 10], bytes[seg_at + 11]]);
        assert_eq!(bytes.len() as u64 - (seg_at + 4 + 8) as u64, size);
    }

    #[test]
    fn inter_frames_carry_block_groups() {
        let mut w = Webm::new(opts(16, 16, 24.0));
        for i in 0..8u8 {
            w.write_frame(&frame(16, 16, 40 + i));
        }
        let bytes = w.finish();
        // 0xA0 (BlockGroup) appears for inter frames; key frames are SimpleBlocks (0xA3).
        assert!(bytes.windows(1).any(|b| b[0] == 0xA0));
        assert!(bytes.windows(1).any(|b| b[0] == 0xA3));
    }
}
