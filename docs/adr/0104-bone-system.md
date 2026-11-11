# ADR 0104: The bone system — skeletons, skinning, IK, smart-bone actions

* Status: accepted (2026-11-11)
* Relates to: ADR 0102 (property channels), ADR 0103 (folder compositing), ADR 0101 (Moho
  import), the Moho parity plan (fork plan › stage 4 — the base-choice trigger)

## Context

Bones are the last hard gap between AniCraft and Moho's core workflow, and the parity plan's
**base-choice trigger**: stage 4's gates decide whether the AniCraft base stands or the
project falls back to the effectcraft route (base-choice-ruling §5). Moho serializes a
`BoneLayer` with a `skeleton` of 61-key `Bone` objects — hierarchy by flat-table index,
per-bone `anim_angle`/`anim_pos`/`anim_scale`/`target_bone` channels (the ADR 0102 channel
model, on bones), angle constraints, IK targets, dynamics, control parents, display flags —
binds artwork to bones per point (`Point.parent`, usually whole-layer `parent_bone` in real
rigs — every CharWalk limb binds by layer), and rides smart-bone actions on the channels
(`actions[]` with pose sub-channels, plus layer-level reference tables).

The corpus holds two bone fixtures, both script-made: bones.moho (five root bones, no
parenting, empty mesh) and smartbones.moho (a dial + target pair, no actions recorded).
Neither binds artwork or records a dial — so the *binding-render* and *dial-frame* gates need
a working fixture from Moho itself, as stage 3's gradients-gui.moho did.

## Decision

1. **Bones live on the layer, addressed like Moho.** `LayerKind::Bone` is a container
   (folder semantics: draws nothing, hides/locks/nests like a folder) carrying
   `Layer.skeleton: Skeleton { bones: Vec<Bone>, binding_mode, extra }`. A `Bone` keeps the
   editable subset as fields — name, parent, length, strength, `pos`/`angle`/`scale`/
   `target` channels, constraints with bounds, hidden, offset, flips — and captures **every
   other key verbatim in `Bone::extra`**, so files round-trip what the model doesn't hold
   yet (dynamics, control parents, physics, display). `Channel<T>` gains `actions:
   Vec<BoneAction<T>>` — named pose timelines for the same property — empty on every
   ordinary channel. MAX_BONES = 2048 bounds every table, walk and command.

2. **Units conjugate once, at import.** Bone channels store stage pixels and
   screen-clockwise radians (ADR 0102 convention): positions map through the inch→pixel
   transform as points, angles and constraint bounds flip and swap with the y axis, lengths
   and offsets scale by the dpi. Evaluation never converts. The chain math is Moho's: a bone
   extends along its local +x by `length`; a root sits at `pos`/`angle` in skeleton space; a
   child hangs off its parent's **tip** with `pos`/`angle` in that tip frame; uniform scale
   propagates to the subtree. Bones never enter the display list: `Skeleton::matrices(frame)`
   computes each bone's world `Affine` (pure, per frame), and bound artwork composes it.

3. **Skinning is pose·rest⁻¹, applied at evaluation.** `skin_matrices(frame)` returns each
   bone's pose relative to its rest pose at frame 0 (FK rest — IK targets that already reach
   at rest must not bend the rig they were bound in), so a rig at rest renders exactly as if
   unbound. A shape carries `bind: Vec<(BoneId, f32)>`: one full-weight bone composes its
   matrix into the display transform (the common case — Moho's single-bone points and
   `parent_bone` layers); several bones deform the path by linear blend (each point the
   weighted average of the bones' matrices — handles translate, no elbow bending). Import
   maps Moho's bindings onto this: all-one-bone shapes get one pair at weight 1; a shape
   spanning several bones draws at rest and is counted (per-point weights are a later
   milestone, sidecar-kept); `parent_bone` binds the whole layer.

4. **IK is the two-bone analytic solve.** A bone with a positive in-table `target` swings —
   with its parent as the second bone — so its tip reaches the target bone's tip: the
   law-of-cosines angles, lengths held, unreachable targets clamped at full stretch, the
   bend side chosen toward the current pose (and the child's relative angle mirrors with the
   side). Solved angles respect constraints. Passes repeat at most 8 times so chains aimed
   at other chains settle; a target *inside* its own chain chases a moving goal and simply
   stops improving (Moho's targets are usually rooted control bones, and our corpus rigs
   bind per layer, not per chain). Scaled or flipped chains don't solve — the FK pose
   stands (noted in code, rare in practice).

5. **Smart-bone actions match by name; the angle pose drives.** Moho's exact dial semantics
   are not public and the corpus records no dial, so this is the documented reading: the
   first action of a name found on an *angle* channel is the driver — its pose values are
   the dial angles each pose frame answers to; every channel carrying an action of that
   name blends its pose (sampled at the frame the driver value inverts to) over its base
   value, linearly, feathered to zero across 10 % of the span outside the pose's value
   range. Moho's layer-level `actions:[{name, pose}]` reference tables and `action_refs[]`
   stay in the sidecar until a working fixture says what they drive.

6. **Commands are the only writer.** `bone.rig_add` (a bone layer + empty skeleton),
   `bone.add` (chains off the last bone; `parent: −1` roots a control bone), `bone.delete`
   (children re-parent to the removed bone's parent; IK targets and shape binds renumber),
   `bone.set`, `bone.key` / `bone.clear_channel` (angle/pos/scale/target keys),
   `bone.bind` / `bone.unbind` (weights normalised), and `bone.info` (the rig read back with
   sampled matrices — what the stage overlay and tests see). All in the `bone` plug-in
   namespace (plug-ins may call them). `.anicraft` is format v7 (older files open
   unchanged); every new field carries a serde default.

## Consequences

* The corpus gates go green with **no skeleton report line**: bones.moho's five bones import
  whole (hierarchy, channels, constraints, 46 untouched keys per bone captured),
  smartbones.moho's dial rig imports, and CharWalk's limb layers bind to their bones.
* The render gate is analytic and pixel-exact: a bound rect rides a 90° bone pose to the
  predicted pixels; the unbound control stays put; a rig at rest renders identically to an
  unbound document. IK is gated analytically (reach, clamp, length hold).
* Deformation allocates per frame only for multi-bone shapes (rare); single-bone binds —
  everything the corpus and real rigs produce today — compose into the existing transform.
* **Open (owner's Moho machine):** the live half of the stage-4 gates — a binding-render
  comparison and a dial-frame comparison against Moho FileRender — needs working fixtures
  created in Moho (the corpus's two are script-made skeletons with empty meshes). The
  sidecar keeps every number meanwhile. The base-choice trigger verdict is recorded in the
  parity plan when those run.
* A bone editor UI (drawing bones on the stage, posing by drag, per-point weights, bone
  dynamics) stays out of scope here; the command layer and rig math are its foundation.

## Corrections from the live gates (2026-11-11)

The corpus fixtures generated for the gates (bones-bind.moho #16, bones-dial.moho #17,
both Moho 14.4 via Lua) corrected three readings in this decision, each caught by a pixel
or analytic gate before it could ship:

1. **A child bone's `pos` is measured in its parent's frame, origin at the parent's
   *base*** (decision 2 said "off its parent's tip"). CharWalk's rig is the evidence: Body
   hangs at CG's base with pos (0, 0), children drawn at the tip store pos.x = the parent's
   length, and sketch_bones.lua maps the drawn base point by the parent rest matrix
   inverse. The tip-frame composition shifted every child by its parent's length. IK is
   re-derived for the base frame: the child's joint swings on a |pos| circle around the
   shoulder (two-circle intersection), which also handles joint offsets past the tip.
2. **Bone bases convert by role.** A root bone's base is a document *point* (canvas-centre
   origin included — the full inch→pixel transform, like the camera track); a child bone's
   base is a delta in its parent's frame (scale and flip, no point mapping). The binding
   gate caught the root case (the posed rig rendered off-canvas); nothing bound to a child
   masked the child case until the dial gate's analytic comparison did.
3. **The smart-bone action rides on the dial's own angle channel as a lookup** — its pose
   values are the dial angles each action frame answers to — and every affected channel
   (bones and layer transforms alike) carries its own pose for the same action name.
   Without the lookup on the dial, Moho never engages the action: every frame renders at
   rest. The name-keyed-driver reading in decision 5 is superseded by the lookup inversion;
   affected channels no longer need the action to sit on their own driver. Action keys must
   be forced INTERP_LINEAR for the frame-by-frame gate — Moho's default smooth in an action
   is not publicly specified, and the approximation shows in pixels.

Gate numbers (translation-aligned F1 on 4px cells against Moho's FileRender, the stage-2
methodology): the binding gate rides a bound canvas through a π rotation at F1 0.257–0.311
(shifts within the ±24px content baseline; the unbound control collapses to 0.164 on the
half-turn frame), and the dial gate follows the smart bone frame by frame at F1
0.227/0.308/0.265/0.274 (f6/f12/f18/f24; stripping the actions collapses the overlap to
0.064–0.073). The posing interaction is real now, too: a Bones tool draws the active
layer's rig on the stage and dragging an IK tip keys the target through `bone.key` — the
drag math is gated analytically (the hand tip lands on the cursor, lengths held, one drag
one undo step).
