//! AniCraft media (layer L0): the built-in media writers behind the export commands.
//!
//! * [`vp9enc`] is a VP9 encoder (profile 0, 8-bit 4:2:0, BT.709) with intra and inter frames,
//!   loop filter and rate control, and the RGB→YUV conversion [`vp9enc::rgba_to_yuv420`].
//! * [`opusenc`] is an Opus encoder (RFC 6716 SILK/hybrid/CELT, 48 kHz, 20 ms packets).
//! * [`webm`] muxes VP9 (+ optional Opus) into a WebM file; [`mp4`] muxes the same codecs into
//!   MP4 (VP9 video and Opus audio are royalty-free, so no patent-pool encoder is bundled —
//!   `docs/legal/compliance.md`).
//! * [`zipw`] writes zip archives with [`deflate`] compression (the `.moho` export package);
//!   [`jpeg`] writes baseline JPEG (the `.moho` package's preview image).
//!
//! Everything here is a pure byte-in/byte-out library: no device, no process, no document
//! model. The engine owns when to encode; these modules only know how.
//!
//! Provenance: the two encoders came from the Craft family's EffectCraft code base
//! (`effectcraft-vp9enc` / `effectcraft-opusenc`, MIT OR Apache-2.0 like this repo), where they
//! were written clean-room from the *VP9 Bitstream & Decoding Process Specification* v0.6,
//! RFC 6716 and RFC 7845, cross-checked against the family's own spec-derived decoders. The
//! muxers and the zip/JPEG writers are AniCraft's own, written from the Matroska/WebM
//! container guidelines and IETF RFC 8794 (EBML), ISO/IEC 14496-12 (ISO-BMFF) with the public
//! "VP Codec ISO Media File Format Binding" and "Encapsulation of Opus in ISO-BMFF" mappings,
//! RFC 1951 (deflate) with the PKWARE APPNOTE (zip), and ITU-T T.81 (JPEG).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod deflate;
pub mod feed;
pub mod mp4;
pub mod opusenc;
pub mod vp9enc;
pub mod webm;
pub mod zipw;

/// The settings both movie containers take.
#[derive(Clone, Debug)]
pub struct MovieOptions {
    pub width: u32,
    pub height: u32,
    /// Frames per second (timestamps and the VP9 rate controller).
    pub fps: f64,
    /// VP9 constant quality 1..=100 (100 lossless).
    pub quality: u8,
    /// A VP9 key frame at least every this many frames (0: only the first).
    pub keyframe_interval: u32,
    /// Opus bitrate in bit/s.
    pub audio_bitrate: u32,
    /// The soundtrack's sample rate (resampled to Opus's 48 kHz on the way in).
    pub audio_rate: u32,
}

impl Default for MovieOptions {
    fn default() -> Self {
        MovieOptions { width: 0, height: 0, fps: 24.0, quality: 60, keyframe_interval: 48, audio_bitrate: 160_000, audio_rate: 48_000 }
    }
}
