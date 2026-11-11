//! The audio side both muxers share: interleaved float PCM at any rate, fed in any chunking,
//! becomes 20 ms Opus packets with their order preserved. Input is linearly resampled to the
//! Opus rate of 48 kHz (a soundtrack's pitch-critical content rides the mix itself, so a
//! deterministic linear resampler is enough here), and the encoder's look-ahead is reported as
//! its pre-skip, which the containers carry.

use crate::opusenc::OpusEncoder;

/// The only rate Opus works at (RFC 7845).
pub const OPUS_RATE: u32 = 48_000;
/// Samples per channel in one Opus packet: 20 ms at 48 kHz.
pub const PACKET_SAMPLES: usize = OpusEncoder::FRAME_SIZE;

/// Feeds interleaved PCM, emits completed Opus packets.
pub struct Feed {
    enc: OpusEncoder,
    channels: u8,
    input_rate: u32,
    /// Whole input frames (interleaved) not yet consumed by the resampler.
    inbuf: Vec<f32>,
    /// Global read position in the input stream, in frames (fractional = resampler phase).
    read: f64,
    /// Global position of `inbuf`'s first frame.
    base: u64,
    /// Leftover 48 kHz samples (interleaved) awaiting a whole packet.
    pending: Vec<f32>,
    /// Packet number emitted so far (each is 20 ms).
    packets: u64,
    finished: bool,
}

impl Feed {
    /// Stereo unless `channels` is 1. `bitrate_bps` is clamped to Opus's range by the encoder.
    pub fn new(channels: u8, bitrate_bps: u32, input_rate: u32) -> Self {
        let channels = if channels == 1 { 1 } else { 2 };
        Feed {
            enc: OpusEncoder::new(channels, bitrate_bps.clamp(6_000, 510_000)),
            channels,
            input_rate: input_rate.max(1),
            inbuf: Vec::new(),
            read: 0.0,
            base: 0,
            pending: Vec::new(),
            packets: 0,
            finished: false,
        }
    }

    /// The encoder delay in samples at 48 kHz: containers skip it on playback (pre-skip).
    pub fn pre_skip(&self) -> u16 {
        self.enc.pre_skip()
    }
    /// The RFC 7845 header the containers carry as the codec's private data.
    pub fn opus_head(&self) -> Vec<u8> {
        self.enc.opus_head()
    }
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Adds interleaved PCM at the feed's input rate; returns completed packets in order.
    pub fn write(&mut self, pcm: &[f32]) -> Vec<Vec<u8>> {
        if self.finished {
            return Vec::new();
        }
        self.inbuf.extend_from_slice(pcm);
        let ch = self.channels as usize;
        let step = f64::from(self.input_rate) / f64::from(OPUS_RATE);
        let have = self.inbuf.len() / ch.max(1);
        // The next output frame reads input samples `i` and `i+1`, so it may be produced while
        // `i + 1 < have`; the last sample is held for the next feed (or flushed by `finish`).
        let mut out = Vec::new();
        loop {
            let i_f = self.read - self.base as f64;
            let i = i_f.max(0.0) as usize;
            if i + 1 >= have {
                break;
            }
            let frac = (i_f - i as f64) as f32;
            for c in 0..ch {
                let a = self.inbuf[i * ch + c];
                let b = self.inbuf[i * ch + ch + c];
                self.pending.push(a + (b - a) * frac);
            }
            self.read += step;
        }
        // Drop what the reader has passed (keep the floor sample: interpolation uses `i`).
        let keep_from = ((self.read - self.base as f64).max(0.0)) as usize;
        if keep_from > 0 {
            self.inbuf.drain(..keep_from.min(have) * ch);
            self.base += keep_from.min(have) as u64;
        }
        out.extend(self.packetize());
        out
    }

    /// Flushes: pads the tail with silence so every sample makes it into a packet (at most one
    /// input frame is still held by the resampler: silence at the very end).
    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        if !self.finished {
            self.finished = true;
            let full = PACKET_SAMPLES * self.channels as usize;
            let pad = (full - self.pending.len() % full) % full + full;
            self.pending.resize(self.pending.len() + pad, 0.0);
        }
        self.packetize()
    }

    /// Emits every whole packet `pending` now holds.
    fn packetize(&mut self) -> Vec<Vec<u8>> {
        let full = PACKET_SAMPLES * self.channels as usize;
        let mut out = Vec::new();
        while self.pending.len() >= full {
            out.push(self.enc.encode_float(&self.pending[..full]));
            self.pending.drain(..full);
            self.packets += 1;
        }
        out
    }

    /// How many packets have been emitted (each one is 20 ms: their timestamps).
    pub fn emitted(&self) -> u64 {
        self.packets
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_are_twenty_milliseconds_and_flushing_pads_the_tail() {
        let mut f = Feed::new(2, 128_000, 48_000);
        let silence = vec![0.0f32; PACKET_SAMPLES * 2 * 5];
        // The resampler holds the last sample back for interpolation, so five packets' worth of
        // input makes four packets now (a hair short of the fifth) and the rest at flush.
        let packets = f.write(&silence);
        assert_eq!(packets.len(), 4);
        assert_eq!(f.emitted(), 4);
        assert_eq!(f.write(&[0.5; 64]).len(), 1); // the held remainder completes a packet
        let tail = f.finish();
        assert_eq!(tail.len(), 2); // the remainder + a padding packet
        assert_eq!(f.emitted(), 7);
    }

    #[test]
    fn resamples_44k1_to_48k_without_losing_length() {
        let mut f = Feed::new(1, 96_000, 44_100);
        let seconds = 2.0f64;
        let pcm = vec![0.25f32; (44_100.0 * seconds) as usize];
        let packets = f.write(&pcm);
        let tail = f.finish();
        let total = (packets.len() + tail.len()) as f64;
        assert!((total * 0.02 - seconds).abs() < 0.05, "{} packets for {seconds} s", packets.len() + tail.len());
    }
}
