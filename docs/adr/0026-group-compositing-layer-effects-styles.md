# ADR 0026: Folder compositing, layer effect stack, style table, four gradient paints

* Status: accepted (2026-11-10)
* Relates to: ADR 0025 (property channels), ADR 0024 (Moho import), ADR 0007 (tiled stage),
  ADR 0004 (destructive-action tiers), the Moho parity plan (fork plan › stage 3)

## Context

Stage 2 brought Moho's property channels to leaf layers and the camera. Three Moho
capabilities were still frozen at import:

* **Groups are composites.** A Moho Group/Bone/Switch layer transforms, fades and filters its
  children *as one*; children keep their own channels on top. AniCraft folders were pure
  organisation: stage 1 baked every container's transform and alpha into the children (so a
  faded group's overlaps wrongly showed through, and an animated group could not be
  represented at all — two skip reports said so).
* **Layer effects.** Moho layers carry `layer_effects` (alpha, blur, noise, pixelation,
  threshold, shadow with expansion/noise/threshold) — a post-stack on the layer's composited
  result. AniCraft had filters only on symbol instances (`ElementKind::Symbol`).
* **Styles and gradients.** Moho fills carry `SS_Gradient2` with four `gradient_type` values
  (0 linear, 1 radial, 2 reflected, 3 angle — all four present in the corpus), a per-shape
  fill-effect transform (`effect_offset/rotation/scale`), per-document named styles
  (`styles[]`, whose 12 built-in brush styles every document carries), and `through_alpha`
  (false = Moho ignores the stops' alpha).

The stage-3 render gates added two corpus facts this decision builds on (CASES.md #15):

* gradients.moho's 13 `has_fill` shapes are zero-area degenerate curves (2 points,
  smoothness 0): Moho itself renders only the four genuine rectangles' strokes, so the
  fixture's gradients are schema, not pixels.
* The working-state fixture (gradients-gui.moho, a 5-point pentagon with the default fill
  stack) pins Moho's **angle-gradient phase: the sweep starts at 12 o'clock and turns
  anticlockwise on screen** — measured from Moho's own FileRender and matched by our
  renderer (dark/light structure F1 0.678/0.754; flattening the gradient collapses it to
  0.144/0.523, the negative control). The same fixture shows a *working* gradient's style
  keys are identical to the degenerate shapes', so geometry — not a style flag — decides
  visibility, and the importer need not special-case it.

## Decision

1. **A folder that composites wraps its run in one display-list group.** `Layer::composites()`
   is true when the layer carries opacity, filters, or (for folders) transform channels.
   `evaluate` then assembles the folder's whole `group_range` run — items keep their own
   transforms chained through the folder's (`transform_at`, stage-2 semantics), the run
   composites at the folder's opacity, and its filters apply after — exactly the
   Diff-doc §13-F option ② (folder upgrades to render semantics; the renderer's existing
   group machinery is reused unchanged; the timeline stays flat). Plain folders keep drawing
   nothing, and old files evaluate identically (identity channels never engage). Nested
   compositing folders stack groups; masks and camera immunity keep their per-layer rules
   inside a run.
2. **Layers carry opacity and a filter stack like they carry transform channels.**
   `Layer::opacity: Channel<f32>` (Moho's `layer_effects.alpha`, 0..=1) and
   `Layer::filters: Vec<Filter>` (≤ 16, sanitized on load) — a leaf layer with either wraps
   its items in one group too. A compositing wrap is frame-independent (`opacity` non-empty
   engages even at full alpha), so evaluations stay stable across playback. Mask artwork and
   guide artwork never wrap: masks ignore opacity, guides never reach output.
3. **Filter variants follow Moho's effect stack, not Animate's menu.** New `Noise`,
   `Pixelate`, `Threshold`; `DropShadow` gains `expansion`/`noise`/`threshold` (serde
   defaults keep old files byte-compatible). The raster effects are deterministic (the grain
   is a fixed per-pixel hash, so renders are reproducible) and analytic-tested; SVG export
   writes only what SVG primitives can express and honestly omits the rest; Lottie reports
   the new gradients as not exported; the GPU stage falls back to the CPU for the new
   gradient paints (`plain_fills` rejects them) rather than drawing them wrongly.
4. **Four gradient paints, geometry = shape bounds placed by the fill-effect transform.**
   `Paint::Reflected` mirrors about the shape's centre (axis centre → left edge, triangle
   wave beyond — verified symmetric), `Paint::Angle` sweeps from 12 o'clock anticlockwise on
   screen (the phase the real-machine gate verified). `gradient_type` maps 0/1/2/3 in
   importer order; `through_alpha: false` forces opaque stops (Moho ignores stop alpha
   then). The importer no longer reports "gradient geometry approximated" — the bounds +
   `effect_offset/rotation/scale` model is what the corpus supports, and linear/radial
   geometry beyond the default remains calibrated by the synthetic analytic gate until a
   working Moho sample exists (open item, not a stage-4 blocker: Moho cannot render the
   corpus's degenerate shapes itself).
5. **The style table is document data, not a rendering path.** `Document.styles: Vec<Style>`
   imports Moho's `styles[]` gated by its `define_*` flags (fill/line colours and width);
   every document's 12 built-in brush styles come across as bare names, brush settings stay
   in the import sidecar (hand-drawn rendering is a later milestone). Shapes keep painting
   their inline styles; a future style binding resolves against the table when one exists
   (the corpus has none).
6. **File format v6.** New document/layer fields ride serde defaults, so v4/v5 files load
   unchanged; v6 marks the first version that writes them.

## Consequences

* Moho groups import as Moho behaves: a transformed, faded or filtered group moves, fades
  (overlaps don't show through) and filters as one, animated channels included — the
  stage-2 skip reports for group transforms are gone.
* The importer's effect mapping is static (channels sampled at the import frame);
  `animated_layer_effects` stays counted until effect parameters become channels
  (the opacity channel already imports key for key).
* The stage-3 gates live in `crates/cli/tests`: `gradient_render_gate.rs` (synthetic
  four-gradient analytics + group-alpha numbers) and `gradients_moho_gate.rs` (the
  real-machine comparisons above; they skip honestly without the Moho renders).
* Open item (stage 4+): real-Moho renders for linear/radial/reflected gradients and for
  animated effect parameters; both need corpus fixtures Moho can actually render.
