# ADR 0107: Built-in encoders, the Chinese interface and the Moho package export

* Status: accepted (2026-11-12)
* Relates to: ADR 0101 (Moho import), 0102–0106 (fork stages 2–6), ADR 0020 (sound), 0040
  (interface languages), ADR 0006 (platform layer); the fork plan (plan › stage 7); the
  compliance policy (`docs/legal/compliance.md` › video and sound)

## Context

Fork stage 7 ("出口与长尾") closes the parity plan's exit gates: export MP4 without a
user-installed ffmpeg, a full Chinese interface, the `.moho` package write-back (zip +
`preview.jpg`), and the long-tail decision points D4 (a Moho Lua compatibility layer) and D5
(the 3D family). AniCraft's video export piped raw frames to whatever ffmpeg the user had; no
ffmpeg exists on a fresh machine, so MP4/WebM export simply failed. The interface spoke
English only (the pseudo-locale proved layout, no real catalog existed, and the bundled Latin
typefaces carry no hanzi). And the fork could read Moho documents but never write one back.

## Decisions

1. **One new L0 crate, `anicraft-media`, carries the media writers.** The VP9 encoder and the
   Opus encoder are migrated from the Craft family's EffectCraft code base
   (`effectcraft-vp9enc` / `effectcraft-opusenc`, MIT OR Apache-2.0 like this repo), where
   they were written clean-room from the *VP9 Bitstream & Decoding Process Specification*,
   RFC 6716 and RFC 7845 and cross-checked against the family's own spec-derived decoders;
   the migration keeps their provenance notes and test suites. The WebM muxer (RFC 8794 EBML
   and the Matroska/WebM guidelines) and the MP4 muxer (ISO/IEC 14496-12, with the public
   "VP Codec ISO Media File Format Binding" and "Encapsulation of Opus in ISO-BMFF"
   mappings), the RFC 1951 deflate writer and the zip writer (PKWARE APPNOTE), the baseline
   JPEG encoder (ITU-T T.81) for the package previews, and the shared PCM→Opus feed are
   AniCraft's own, written from those public specifications. No new dependency (miniz_oxide
   and zune-jpeg are dev-dependencies of the round-trip tests, already in the tree).
2. **`file.export_video` uses them and the ffmpeg path is gone.** Both containers are VP9
   video + Opus sound (royalty-free; no patent-pool codec is bundled, per the standing
   policy). Frames render in parallel batches as before, the soundtrack mixes in blocks
   straight into the encoder (the temp-WAV hand-off is deleted), odd sizes are padded to
   even, and overwrites still confirm. MP4 here means VP9-in-MP4 with Opus — H.264/AAC stay
   out of the tree, and the compliance doc says so.
3. **Chinese ships with the fork.** `locales/zh-CN/anicraft.tsv` (one row per interface
   string, 1540 rows) is vendored in-repo and embedded when no `CRAFT_I18N_DIR` checkout is
   given (the env var still wins), so builds carry Chinese by default. A Chinese system
   locale resolves to it (`zh-cn` in the offered list). The strings were written clean-room
   from the plain English meaning by translation sessions working from a shared glossary; no
   other product's localization was consulted. Because the bundled faces are Latin-only, the
   theme installs the system's CJK face as the last fallback of both font families while the
   language needs it (Hiragino Sans GB / STHeiti / PingFang on macOS, Microsoft YaHei on
   Windows, Noto Sans CJK / WenQuanYi on Linux) — read at runtime from the OS install, never
   bundled, the same rule the user fonts of Preferences follow.
4. **`file.export_moho` writes a Moho-openable package.** `.moho` = zip(`Project.mohoproj` +
   `preview.jpg`, the corpus's own container form, written by our zip writer); `.mohoproj` =
   the bare JSON with the preview embedded as `thumbnail`. Two paths by origin: an imported
   document exports its sidecar raw with the model's current state over it (the import's
   flatten order is deterministic, so raw layers pair with model layers positionally) — the
   original mesh with its true handles and every unknown key ride through; a native document
   generates the canonical template (extracted once from the corpus's fresh minimal save and
   kept as data) and draws shapes into Moho inches with handles in the form the importer
   reconstructs them from. The gate is export → import: nine corpus fixtures re-import to the
   same model, and a generated native document round-trips its geometry, fill and structure.
   Whether Moho 14.4 itself opens the package stays an owner-side real-machine check (R2),
   like every stage before.
5. **D4 is closed: no Moho Lua compatibility layer.** AniCraft's automation surface is the
   command registry over the CLI, JSON-lines, the MCP server and the WebAssembly plug-in host
   (the MCP-first line the plan already recommended); a Lua interpreter would be a large,
   open-ended subsystem serving scripts written for another program. A JSON→Lua adapter
   script remains the migration bridge for owners of existing Moho scripts.
6. **D5 is closed: the 3D family stays out of the 1:1 scope.** `3d_mode`, `3d_options`,
   depth-of-field, stereo and `perspective_shadow` keep riding the sidecar untouched; the 2D
   fields remain the gauge. This matches the plan's recommendation and effectcraft's own
   gap analysis.

## Consequences

* Video export works on a machine with no ffmpeg; the encoder stack is first-party family
  code with the same licence, and `ATTRIBUTION`/compliance records the migration.
* The fork's default build speaks Chinese end to end (menus, panels, dialogs, status bar),
  verified by `--lang zh-cn` screenshots; other locales stay reachable through
  `CRAFT_I18N_DIR` checkouts.
* Documents can leave AniCraft as Moho packages — the L3 dialect goal ("Moho opens it") now
  has a writer to test on the machine. Exported native meshes carry AniCraft's handle form;
  corpus-fidelity meshes (an imported document's) ride the sidecar, so nothing degrades on
  the round trip.
* `panic_hunt_every_command` fuzzes the two new commands; the layering test carries
  `media` at L0; the workspace map lists it.
