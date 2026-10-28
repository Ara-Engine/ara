# Preview 4 release-note handoff

Draft notes for Engine and Hub **0.0.2-alpha.4** are in
[RELEASE_NOTES.md](../../RELEASE_NOTES.md) and
[HUB_RELEASE_NOTES.md](../../HUB_RELEASE_NOTES.md). They are user-facing copy;
this file records provenance, pending claims and packaging handoff.

## Source boundary

- Baseline: `engine-v0.0.2-alpha.3`, `029563864b88cb1d5746481e731cf9c78e67212d`.
  The alpha.3 notes identify source `237d8511b2ec10a226941fcbb556a893160813bc`.
- Draft branch base: origin main `0059cf3b15bed647132aa9d16a16a4c3609fc84c`.
- Local main reviewed through `9fdf6bf74`, including fidelity `c708ac0bc`,
  J10 `6456f260e`, Metal fixes through `cc09c9e9` and example/golden fix
  `dfbf80223`. These changes are described but not merged into this docs branch.
- Queued changes included in the draft: `t3/export-runtime-records` `1561cd9d9`,
  `t3/samples-mirror` `dcf889e63` and `t3/metal-rt-verified` `7628fe102`.
  The last already includes the export-runtime-record changes; count them once.

Commit subjects and first-parent merge messages were reviewed across the range,
with implementation/docs checks for the claims below. Existing features in the
alpha.3 source are not presented as new merely because the old generic preview
notes did not enumerate them.

## Claim sources

| User-visible change | Reviewed commits / merge records |
|---|---|
| `content/`, layout v4, consent, typed migration, ARA1914/ARA1915 and engine pinning | `10cc6330b`, `60ed0850c`, `d410ba193`, `5fb786d1b`, `b6ca1fd86`, `a0df44a8b`; `docs/project-layout.md` and diagnostic catalog |
| Asset import/organization, reference and companion handling, reversible files/folders | `35741c5a3`, `824fac06d`, `fd18de2f3`; asset command README |
| Offline history, entity revert, headless services, render-aware why | `8e813f096`, `105f092b8`, `83c368780`, `a3562cc3b`; host and headless-service contracts |
| Canonical file-write echoes preserve Redo; actual external edits still recorded | `c25eeb88e`, `518910525` |
| Progress-based CLI job waits | `042d7ca97` |
| C# editor API, hierarchy/destruction, installed code fixes and undoable source edits | `0dbcb24c6`, `8498eccce`; compiled editor example and installed-layout tests |
| Module inventory, generated schema publication before validation, Play regressions | `f17aa8ff6`, `40673e0c7`, `57e43b131`, `b72838d16`; current content-root contract |
| J10 compiler ownership/completion and scoped Play diagnostics | `6456f260e`, `b92ba096c`; Problems/Console projection remains pending editor work |
| Signed installs/repair, consent-based updates, removal inspection and Create page | `d21f39bd2`, `015395071`, `04a305b2a`, `f9bd83e08`, `857a6aee0` |
| Shared selection/menu/dialog behavior and grapheme-safe wrapping already merged | `fedbfaabc`, `61efced5f`, `653996b8b`, `9ef70ff0d`, `a5f0c4f3b`; later editor work is not assumed landed |
| Named exports, isolated upload folder, preflight/repair and relocated data lookup | `2519c0266`, `369f98474`, `6bde7888f`; export README |
| VFX export effect records and atlas | `9bcc42720`, `1561cd9d9` |
| Kenney mirror fallback with unchanged integrity/credits | `dcf889e63`; real-network empty-cache fetch/verify passed before this draft |
| Frames in flight, bounded presentation waits and native indexed draws | `8c0400bcb`, `ce03474e6`, `ec198d712`, `f302f49c9`, `c3d741c98`, `d044d45ac` |
| TSR, AgX, glass, material filtering, reflection and occlusion changes | `c708ac0bc`, `10247670a`, `4705d0b94`, `cd9978dd2`, `cf9c7c1ad`, `92e151c70`, `e4d1f683f`; final fidelity source, not isolated intermediate benchmark results |
| Reflection captures are experimental/off by default | `41d524ad5`; `7628fe102:crates/ara-render-fidelity/src/presets.rs` |
| macOS source builds, Retina input, native crash support and Mac-built app exports | `7509cf13c`, `9c6633e1f`, `292396f20`, `cc1e930b4`, `6ba797cea`; macOS guide and export README |
| Crash-monitor startup with long Windows temporary paths | `f3a5f2b1b`, `2cd6c0dee` |
| Metal emitter-speckle fix, verified ray queries and explicit unsupported-RT fallback | `1354422e9`, `cc09c9e9`, `349dce851`, `46f4259e9`, `7628fe102`; ARA6008 |
| Fidelity Lab and dev-only Rust body hot-patching | `52a77e5ce`, `232ba1138`, `b7f592281`, `9dd7940d2`; `docs/dev/ara-dev.md` |
| Matched Showcase cross-engine adapters, without claiming a winner | `b7a6b324a`, `81c0b8e95` |

The renderer distinction matters: High/Epic's D29 hybrid can use verified hardware
ray queries, while the separate fidelity renderer is an explicit RT opt-in.
Do not describe all High/Epic rendering as raster-only, or present the opt-in
tier's glossy grain as fixed by the Metal reservoir initialization.

## Still landing

| Draft section | Required final input |
|---|---|
| TODO — J7 | Final `t3/vq-j7` merge, actual behavior and checks; no guessed feature description |
| TODO — sun-bounce fix | Final `t3/vq-sunbounce` merge and reference comparison; decide what shadowed-ground limitation remains |
| TODO — editor follow-up | Final `t3/ux-editor-p4` merge and native journeys; confirm resolved C# errors retire from Problems and Console retains history |
| TODO — formal measurements | Final-source report after those merges, with valid environment and complete expected pass coverage |

Keep the RT two-sided-panel bug in Known issues unless its specific fix lands and
passes its regression. The existing double-sided G-buffer and glass changes do
not establish that this RT issue is closed. Glossy grain in the opt-in RT tier is
an explicit known issue on both backends.

## Claims awaiting numbers

The draft makes no quantitative speedup or budget-pass claim. Fill or omit each
of these publication slots from the final formal reading, never from alpha.3's
table or an intermediate shader microbenchmark:

| Claim slot | Numbers and context needed |
|---|---|
| Rendering speed / D26 and D27 outcomes | Outpost, Hangar and Showcase GPU p50/p95 for Low/Medium/High/Epic at 2560×1440 on the reference RTX 4070 SUPER; separate raster/hybrid from opt-in RT; matched alpha.3 readings for any delta or percentage |
| CPU cost and frame pacing | Play CPU work p95, editor Scene/orbit work, and counts/maxima of frames over 16 ms after the first; identify cold/warm runs and which budget is being assessed |
| Startup | Cold first presented loading frame and warm first fully ready scene frame for player and editor; keep asset-ready/import completion distinct from a complete rendered frame |
| Editor responsiveness during imports | Frames above 16.7 ms during first import and after settling, plus input-to-frame click and drag p95; use the final editor/J7 build |
| Import completion | Source count, elapsed assets-ready time and cache conditions on the final source; distinguish network acquisition, project import caches, and unflushed OS/driver caches |
| Image quality accompanying the timings | Per-preset FLIP/RMSE against the path-traced reference; final region comparisons against Cycles where claimed, with tone mapping, resolution, render scale, samples and motion/stability conditions |
| macOS performance, if quantified | Exact Apple model, OS, power/validation state, path/preset/resolution, GPU/CPU statistics and startup; do not apply RTX budgets or intermediate M5 percentage wins to this hardware |

Fidelity Lab shader-save-to-frame, A/B turnaround and Rust hot-patch/restart
latencies are separate development-workflow measurements, not D26 output. The
draft advertises the workflows without timings. Any added seconds or comparative
claim needs a final-source matched workflow report; old `<2 s`, `<5 s`, `<20 s`
and warm-cache receipts are not release-wide promises.

## Packaging handoff

1. Resolve the four TODO sections and the RT panel issue disposition, then freeze
   both reviewed notes against the actual release source. Recheck the claims if
   a queued branch changes before merging.
2. Engine `ara-release notes`/packaging copies root `RELEASE_NOTES.md` verbatim and
   requires its first line to be exactly `# Ara 0.0.2-alpha.4`.
3. **Preview automation currently replaces the root notes:**
   `ara-dev/src/preview/version.rs::bump` writes a generic source-commit summary
   into its isolated release clone. Arrange preservation of the reviewed draft
   before packaging, or use the reviewed-source manual packaging path in
   [release/README.md](../../release/README.md). A successful automatic preparation
   alone does not establish that these reviewed notes shipped.
4. **Hub notes are generated separately:** `ara-release/src/hub.rs` currently
   writes a generic `HUB_RELEASE_NOTES.md`; it does not read the new root Hub draft.
   Hand the reviewed Hub copy to the release-preparation owner before packaging
   and signing. This docs-only branch changes neither generator.
5. Verify the actual packaged engine `RELEASE_NOTES.md`, Hub `HUB_RELEASE_NOTES.md`
   and their signed manifest/publish copies. Both include the Authenticode warning.
   Do not replace or regenerate notes after signing a reviewed release folder.
6. Record the final release-source SHA and formal report identity. The notes and
   this handoff do not bump versions, prepare artifacts, tag, push or publish.

No automated documentation test reads these root release-note files. Check their
headers, local document links, required draft sections and whitespace without
building Cargo targets; installed-package smoke and final release gates remain
the integrator's responsibility.
