//! Audio layers (ADR 0106): a Moho `AudioLayer` becomes a sound in the library and a sound
//! on the layer's keyframe, following the upstream audio model (ADR 0020/0039) — sounds live
//! in the document, belong to keyframes, and play tied to the timeline (Stream sync, which is
//! how a Moho audio layer plays). The layer's file is read through `audio_path` (the official
//! manual's audio-layer path key) or its `audio_fileref`; WAV, Ogg Vorbis and MP3 decode, and
//! anything else is counted, with the sidecar keeping the file untouched.

use crate::import::Builder;
use crate::json::{self, get};
use anicraft_doc::{FrameSound, Keyframe, Layer, Sound, SoundId, SoundSync};
use serde_json::Value;
use std::sync::Arc;

/// Fills an audio layer: the file becomes a library sound and the layer's keyframe sound.
/// Nothing draws; the keyframe spans the sound's length (capped at the project range), the
/// way Moho's timeline shows an audio bar.
pub(crate) fn audio_layer(b: &mut Builder, layer: &Value, l: &mut Layer) {
    let file = json::string(get(layer, "audio_path")).filter(|p| !p.is_empty()).or_else(|| {
        get(layer, "audio_fileref").and_then(|f| json::string(get(f, "path"))).filter(|p| !p.is_empty())
    });
    let Some(file) = file else {
        b.report.skip("audio layers without a file reference");
        return;
    };
    let bytes = match (b.read)(&file) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            b.report.skip("missing audio files");
            return;
        }
        Err(e) => {
            b.report.notes.push(format!("audio file unreadable: {e}"));
            return;
        }
    };
    let short: String = file.rsplit(['/', '\\']).next().unwrap_or(&file).chars().take(256).collect();
    let sound = match Sound::from_data(SoundId(b.doc.alloc_id()), short, bytes) {
        Ok(s) => s,
        Err(_) => {
            b.report.skip("audio files that are not WAV, Ogg Vorbis or MP3");
            return;
        }
    };
    let frames = sound.movie_frames(b.doc.stage.fps).min(b.duration).max(1);
    let id = sound.id;
    b.doc.sounds.push(Arc::new(sound));
    let mut k = Keyframe::new(0, frames);
    k.sound = Some(FrameSound { sound: id, sync: SoundSync::Stream, repeat: 1, looping: false, effect: Default::default() });
    l.keyframes = vec![Arc::new(k)];
}

/// The document soundtrack (`project_data.soundtrack`): a file path, imported as a library
/// sound — its placement in the movie needs a Moho specimen to pin down (the corpus has no
/// soundtrack yet), so the note says what happened instead of guessing a slot.
pub(crate) fn soundtrack(b: &mut Builder, path: &str) {
    let bytes = match (b.read)(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            b.report.notes.push("the soundtrack file is missing (kept in the sidecar)".into());
            return;
        }
        Err(e) => {
            b.report.notes.push(format!("the soundtrack file is unreadable: {e}"));
            return;
        }
    };
    let short: String = path.rsplit(['/', '\\']).next().unwrap_or(path).chars().take(256).collect();
    match Sound::from_data(SoundId(b.doc.alloc_id()), short, bytes) {
        Ok(sound) => {
            b.doc.sounds.push(Arc::new(sound));
            b.report.notes.push("the soundtrack imported into the library; its movie slot follows a Moho specimen".into());
        }
        Err(_) => b.report.skip("soundtrack files that are not WAV, Ogg Vorbis or MP3"),
    }
}
