# ADR 0108: Moho group masking and the drawn mask

* Status: accepted (2026-11-13)
* Relates to: ADR 0101 (Moho import), ADR 0103 (group compositing), the Moho parity plan
  (fork plan), the maop-corpus (keyframes-camera.moho, CharWalk.mohoproj)

## Context

The corpus audit (every fixture plus CharWalk, the fork's flagship sample) left exactly one
mask-shaped difference: keyframes-camera and CharWalk carry Moho's group masking — the rig's
eyes — and the importer drew the members unmasked and counted them. The evidence for the
semantics is all public: the Moho Pro manual's masking tutorial names the modes, and the
vendor's public scripting reference pins the numbers — `MOHO.GROUP_MASK_NONE` = 0,
`GROUP_MASK_SHOW_ALL` = 1 ("turn on masking, initially showing all sub-layers"),
`GROUP_MASK_HIDE_ALL` = 2 ("…hiding all sub-layers"); per layer, `MOHO.MM_MASKED` = 0 (a
masked member), `MM_NOTMASKED` = 1 (ignores masks), `MM_ADD_MASK` = 2 ("add to the current
mask" — the layer draws itself and defines the region), `MM_SUB_MASK` = 3, `MM_ADD_MASK_INVIS`
= 4 ("add, but keep invisible"), `MM_SUB_MASK_INVIS` = 5, `MM_CLEAR_ADD_MASK` = 6/7. The
corpus renders confirm the reading: the whites' layer carries `masking` 2 and Moho's own
preview shows it drawn, with the pupil clipped inside it.

AniCraft's mask layer (Animate-style) never draws its fills: they only clip the layers
parented to it. Moho's common idiom — real rigs' eyes — is the drawn form (MM_ADD_MASK), so
restoring it 1:1 needs the mask's fills to draw as well as clip.

## Decision

1. **`Layer.mask_draws: bool`** — a mask that also draws itself, beneath the layers it
   masks (serde default off; `.anicraft` v4 files and every existing document unchanged).
   Evaluation is where it lives: a drawn mask's artwork is evaluated twice — the clip pass
   as always, then its fills again as plain artwork — and the drawn copy moves in the
   display list to just before the first item of the layers it masks (the bottom-up walk
   painted them before the mask itself, so the copy lands under them; renderers collect a
   clip's sources regardless of item order). Without masked artwork the copy stays at the
   mask's own place in the paint order. Renderers, the GPU stage and every export consume
   the display list, so nothing downstream changes: the drawn copy is ordinary items, the
   clip pass is the existing clip source.
2. **The importer maps the whole enum.** A `group_mask` 2 group walks its children through
   the mask roles: a mask member (2/6) imports as a mask with `mask_draws`, an invisible
   one (4/7) as a mask alone, the masked members (0) as its children (mesh/image/text
   only — `repair_masks` keeps nothing but normal layers under a mask), unmasked members
   (1) draw beside it, and a "keep invisible" subtract (5) imports hidden. What has no slot
   is counted, never silently dropped: subtracting (3/5) cuts nothing, mask members past
   the first don't reach the single clip, a mask role on a container or a content-less
   layer does nothing, and containers cannot sit under a mask. A `group_mask` 1 group
   ("show all") shows everyone; only its invisible forms stay invisible, its subtracts are
   counted. `mask_expansion` is counted when it carries a value. A hiding group with no
   region at all imports its members hidden — exactly what Moho shows.
3. **The gate is the file's own geometry.** keyframes-camera has no Moho FileRender PNG,
   so the render gate (`crates/cli/tests/mask_render_gate.rs`) checks the rendered eye
   against the mask layer's own fills — the white draws, the pupil draws only within the
   fill's reach (past its own outline stroke nothing dark appears) — and proves the checks
   can fail: the pupil's items carry the mask's clip (unparenting removes it), and turning
   `mask_draws` off (Moho's MM_ADD_MASK_INVIS form) makes the white vanish while the pupil
   stays clipped. The corpus fixture test pins the imported structure; a doc-crate unit
   test pins the evaluation order. Moho's own render of this fixture stays with the
   corpus's real-machine probes.

## Consequences

* Moho's eyes — and every rig masked the same way — open drawing exactly as in Moho: white
  over the head, pupil inside it. The corpus audit's mask line disappears; what remains is
  reported, not drawn.
* The drawn copy doubles the mask's artwork items on drawn masks only (an eye white: two
  items); invisible masks cost nothing extra.
* Moho's subtract masks, multi-mask unions and mask expansion still have no slot and are
  counted; the sidecar keeps every value for the later milestones that take them.
* Timeline editing of a drawn mask edits draw and clip together — one layer, as in Moho.
  A later milestone can expose the flag in the timeline UI; imported documents carry it
  either way.
