# ADR 0109: Follow Path — a layer rides another layer's curve

* Status: accepted (2026-11-13)
* Relates to: ADR 0101 (Moho import), ADR 0102 (property channels), ADR 0104 (bone
  system), ADR 0103 (group compositing), the Moho parity plan (fork plan), the maop-corpus
  (follow-path.moho, CASES #10)

## Context

The corpus audit (23 documents, `anicraft-mohoproj-import --example audit`) counted the
Follow Path family as its largest animation gap: follow-path.moho carries a real relation —
an empty "FollowLayer" follows Layer 1's S curve (`follow_layer_uuid` + `follow_curve` 17),
turned along it (`rotate_to_follow`), with the progress channel animated (1.0 → 0.0 over
frames 0 → 50) — and the importer drew nothing of it.

The semantics are public: the scripting API's `SetFollowingCurve(layer, curve,
startingCurvePercentage, bendWithCurve)` names the third argument a percentage and the
fourth the bend flag (the file's `follow_bending`), and the Follow Path tool's percentage
value is the position along the curve (0 = the curve's first point). The corpus confirms the
shape: `transforms.following` is a plain animated `Val` channel in curve percentages.

## Decision

1. **`Layer.follow: Option<FollowPath>`** — the relation rides the document: the target
   layer, the followed curve resolved into an element id and subpath, the animated
   `following` channel (curve percentage, Moho's own values), `rotate` and `bending`.
   Serde-default off; `.anicraft` v4 files unchanged.
2. **Evaluation places the layer.** `doc::evaluate` gives a followed layer the path point
   — and the tangent's angle when it rotates to follow — ahead of its own transform
   channels, which then move it relative to the path frame. The curve geometry is the
   target's evaluated elements (its folder chain and transform channels compose in; the
   follower's own camera and folder chain then apply as for any layer). The arc position
   measures on the flattening, the renderer's own geometry. A relation that cannot resolve
   (target gone, curve element gone) leaves the layer where its own transform puts it.
3. **The followed curve flattens onto its target as an invisible shape.** Moho's followed
   curve may belong to no shape of its own (the corpus's S curve does not) — the importer
   adds it to the target layer's keyframe as a path with no fill and no stroke, draws
   nothing, and hands its element id to the relation.
4. **Bending is kept and counted, not guessed — and the count is machine-confirmed.**
   `follow_bending` (the scripting API's `bendWithCurve`) warps the artwork along the path
   in Moho; the vendor publishes no algorithm. The installed Moho Pro 14.4 has now shown
   what it does (maop-corpus CASES #23, `follow-bend-on/off.moho` + 22 FileRender PNGs):
   with bending off the `following` channel drives travel (every frame differs, rigid
   placement along the curve); with bending on the layer's contents warp to conform along
   the curve and the channel acts as a near-binary influence — following 1.0 renders the
   layer at its authored place, any value below renders the conformed warp byte-identically
   (frames 5–50 identical at values 0.92…0.0). The warp maps each shape along the curve
   with tangent rotation; the attach point is not in the file — Moho recomputes it from the
   layer's origin. AniCraft's percentage-travel evaluation matches Moho's bending-off mode
   qualitatively but not the bending-on mode the corpus's fixture actually uses. Restoring
   the warp means reverse-engineering that unpublished mapping; the ground truth renders are
   archived in the corpus for a later stage to approximate against a real-machine gate.
   Until then the flag rides the document and the audit counts "follow bending" — exactly
   the risk register's rule: unprovable parameters stay beside, not rendered.

## Consequences

* follow-path.moho animates: the follower rides the S curve across its 50-frame progress,
  turned along it — gated by the file's own geometry (`follow_path_render_gate.rs`: at
  progress 0 the follower centres exactly on the curve's first point, an expectation that
  needs no handle formula; progress 1 % steps off it; the item matrix turns by the first
  segment's tangent; the followed curve draws nothing).
* The audit's follow-path union (relation, progress animation, rotate to follow) drops to
  the single bending count.
* A followed layer inside transformed folders composes its own folder chain on top of the
  target-chain geometry — exact when both share the folder (the corpus case, both
  top-level); Moho's exact cross-folder composition stays with the runtime probes.
