# ADR 0106: Media and simulation — image warp, audio layers, physics baking

* Status: accepted (2026-11-11)
* Relates to: ADR 0101 (Moho import), ADR 0102 (property channels), ADR 0105 (point-level
  channels), ADR 0020/0039 (sound library, keyframe sounds), ADR 0032 (GPU stage), the Moho
  parity plan (fork plan › stage 6) and its decisions D2 (audio follows upstream) and D3
  (a self-built physics integrator)

## Context

Stage 6 closes the parity plan's media and simulation group: **image deformation** (Moho
deforms an image layer through its mesh), **audio layers** (an `AudioLayer`'s file playing
tied to the timeline, and the document soundtrack), and **physics** (Moho's layer rigid
bodies — enabled groups carry a world with gravity and wind; Moho itself bakes the result
into the layer's `physics_nudge` channel, which `use_baked_physics` then plays).

The evidence base: the corpus has no deformed image mesh, no `AudioLayer`, and no animated
`physics_nudge`; what it does have is real-file evidence of the field shapes — the Lute
startup project's 28 image layers carry `width`/`height` (the drawn size in inches) and
`psd_layer_translation`, and fresh-minimal's layer carries the full `physics` group
(density 1, friction 0.3, restitution 0.5) with `physics_nudge` as a `Vec2` channel, while
CharWalk's groups carry the default gravity (0, −10) inches/s², y up.

## Decision

1. **The image warp is a texture pin, not a renderer feature.** `Shape.texture:
   Option<Box<TextureMap>>` holds the image and the image-pixel coordinate of every path
   point. Evaluation — which already rebuilds deformed paths (ADR 0105) — derives the fill
   from the pin each frame: the affine fitted from the path's points onto the pinned
   coordinates (`fit_affine`, exact for three points), written as the `Paint::Bitmap` the
   renderer, the GPU stage and every export already draw. Zero renderer changes. The Moho
   importer splits a mesh shape's outline into triangles by ear clipping (straight edges;
   handles have no place in an image cage), so each piece maps exactly at every corner, and
   animated mesh points ride their triangles as ADR 0105 point tracks — a corner shared by
   two triangles carries the same channel, so the seam stays welded. An undeformed image
   keeps its plain rectangle, now sized by the layer's own `width`/`height` and offset by
   `psd_layer_translation` (real-file evidence). Deformer switches, cropping and movie
   layers are counted.
2. **An audio layer is a library sound and a keyframe sound.** The file (`audio_path`, or
   its `audio_fileref`) decodes into the document's sound library (ADR 0020) and rides the
   layer's keyframe as a Stream sound spanning the sound's frames — a Moho audio layer plays
   tied to the timeline, which is exactly Stream. The document soundtrack imports as a
   library sound with a note: its movie slot needs a Moho specimen to pin down. Files that
   don't decode (AIFF, …) are counted, never silently dropped.
3. **Physics bakes; evaluation stays pure.** A scene's world comes from Moho's
   physics-enabled group (`enable_physics` with `gravity`/`wind`, converted inches/s²
   y-up → stage px/s² y-down; the corpus's default gravity (0, −10) becomes (0, +3200)).
   Layers with `physics.enabled` carry `PhysicsBody` (static, friction, restitution, start
   velocity); density ratios, respawning, sleeping, pivots, motors and force fields are
   counted, not held. `physics.bake` (D3: a self-built integrator, no rapier) simulates once
   with symplectic Euler at the frame step — bodies fall, land and bounce on static layers'
   boxes (artwork bounds), friction slows the slide, settled bodies sleep — and keys each
   dynamic body's translation per frame, after which every renderer, export and the timeline
   show the motion with no further machinery. That is Moho's own `use_baked_physics` idea;
   Moho's already-baked `physics_nudge` channel merges into the translation on import (sums
   at the union of key frames, re-keyed linearly and reported when easing says otherwise).
   A layer whose translation already animates refuses the bake; body-vs-body collisions and
   rotation wait for a Moho specimen that needs them.

## Consequences

* The format moves to v9: older builds refuse stage-6 files instead of silently dropping
  the texture pin, the body and the world.
* The warp's approximation is per-piece affine: interior mesh points (a cage denser than its
  outline) are not yet Steiner points, and a multi-bone skin bending a textured path doesn't
  carry the pin's matrix with it — both documented, both visible only in files that don't
  exist in the corpus yet.
* Physics units are stage pixels per second at the document's frame rate; the real-machine
  gate (a Moho drop fixture against our bake) is what pins Moho's gravity scale to ours.
* The `physics` command namespace joined the plug-in policy (test-locked); the panic hunter
  fuzzes `physics.bake`/`physics.info` with the rest.

## Gates

In-repo (all green): texture fit exact on triangles, degenerate sets refuse; evaluation
refits the pin per frame (doc tests); the oracle renderer samples the warped image where
the pin says, at rest and mid-stretch (render tests); importer quad-warp with a shared
animated corner, plain-image sizing, audio layer → Stream keyframe sound, AIFF counted,
soundtrack noted, physics world/body/nudge merge (importer tests); ballistic closed form,
landing, restitution bound, friction stop, determinism (physics tests); command bake, undo,
refusals (engine tests); the stage-6 model round-trips the native format whole. Corpus
fixtures all import as before.

Real-machine (pending, owner's Moho 14.4): fixtures generated by Lua for a warped image
layer and an audio layer, and the drop sample — Moho `FileRender` frames against our
imported/baked render, the stage-2/4/5 F1 method.
