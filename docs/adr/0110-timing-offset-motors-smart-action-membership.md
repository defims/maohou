# ADR 0110: Timing offsets, motor keys and smart-action membership

* Status: accepted (2026-11-13)
* Relates to: ADR 0101 (Moho import), ADR 0106 (physics baking), ADR 0104 (bone system),
  ADR 0039 (keyframe sounds), the maop-corpus (audio-track.moho CASES #22, physics-drop
  CASES #20, image-warp CASES #21, keyframes-camera)

## Context

Three audit lines remained after the masking, follow-path and motor work: a layer timing
offset on audio-track.moho, "physics motors" on both physics fixtures, and 44 "smart bone
action references" across keyframes-camera and CharWalk. Each turned out to be a different
kind of difference — a placement, a false positive, and unmodelled bookkeeping.

## Decision

1. **The audio layer's timing offset places its sound.** Moho's `timing_offset` is the
   scripting API's "number of frames between the layer's frame zero and timeline frame
   zero" (`SetTimingOffset`, "moves the layer start time"). The corpus pins the sign
   without a runtime probe: audio-track.moho's generation log imports the WAV at 1-based
   frame 24 and Moho saved `timing_offset` −23 — the negation of the 0-based start. The
   audio layer's keyframe sound starts at −offset frames, where Moho's sequencer shows it.
   A positive offset (the sound head before the timeline) is counted — AniCraft's frame
   sound has no head-trim slot; content-layer offsets (no specimen shifts them) are
   counted as a shifted channel timeline.
2. **An off motor with constant keys is not a motor.** Moho writes constant default motor
   keys on every physics body — the corpus's two motor fixtures carry two keys with the
   same π and `enable_motor` false — so the animation check (more than one key) counted
   motors no file ever turns on. A motor counts only when it is on or its speed genuinely
   varies over time; the corpus bodies behave exactly as Moho runs them, uncounted. An
   enabled motor still has no bake slot (ADR 0106's integrator is a rigid-body bake) and
   stays counted.
3. **The layer's smart-action membership rides the document.** Moho stamps every layer
   inside a rig with `actions: [{name, pose}]` — keyframes-camera's 21 layers all carry
   the same two entries (`Eyes_LR`, `Eyes_UD`, pose 0). The poses themselves live on the
   skeleton's channels, which stage 5 imports and the rig plays; the per-layer array is a
   membership record, not animation. `Layer.smart_actions` holds it (name + pose id),
   serde-default empty, so a Moho rig's bookkeeping survives in the model instead of only
   the sidecar, and the audit line counts nothing.

## Consequences

* The corpus audit's union is one line — "follow bending" (ADR 0109's counted flag). Every
  other difference the audit found across the 23 documents is imported, counted nowhere.
* Fixture gates pin all three: the audio bar starts at 0-based frame 23; the physics
  fixtures import without a motor line and a constant-key body stays uncounted; the rig's
  membership table arrives on every layer.
* Moho specimens that turn a motor on, trim a sound head by a positive offset, or shift a
  content layer's channel timeline will surface as counted lines again — the report names
  them, the sidecar keeps them, and a later stage takes them with their own gates.
