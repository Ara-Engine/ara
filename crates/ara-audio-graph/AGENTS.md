# ara-audio-graph

Procedural audio graphs (MetaSounds-class, smaller surface): one typed AST with a
compact ID-free text view, an `ara-graph` projection (domain `audio`), typed undoable
commands, and a compiler to a flat, allocation-free DSP program that runs as an
`ara_audio::SignalSource` voice.

## Text (`.audiograph`)

```text
graph Footsteps {
  param surface = 0 range(0, 2) "0 concrete, 1 grass, 2 metal";
  trigger step "Fired on each footfall";
  let concrete = random.pick(step, 11, @sfx/step_c1.wav, @sfx/step_c2.wav);
  let grass = random.pick(step, 12, @sfx/step_g1.wav, @sfx/step_g2.wav);
  let clip = switch(surface, concrete, grass);
  let pitch = random.range(step, 14, 0.94, 1.06);
  let player = sample(clip, step, pitch);
  output out = player.out;
}
```

- One operation per `let`; arguments are names, numbers or `@clip` references.
  Positional in the op's input order, `_` leaves an optional input unconnected, named
  (`lowpass(x, q: 2)`) after positionals. `name.port` picks an output; multi-output
  nodes always print their port. Types: `audio`, `trigger`, `clip` (`let x: audio = ...`).
- `param` (number with `range`, or `@clip`), `trigger`, `output out|left|right|done`.
  `done` ends the voice (one-shots). Canonical printing: params, nodes in dependency order
  (author order breaks ties), outputs. `graphs/*.audiograph` are canonical examples:
  footsteps by surface, wind, engine RPM, UI click (stereo), rifle with tails.

## Public API

- `dsl::{parse, reconcile, print}`; `reconcile` keeps node IDs for surviving names.
  New IDs are `ast::mint_id(name)` (BLAKE3, deterministic).
- `check` → `Checked` (types incl. generic `switch`/`random.pick`, required and
  literal-only inputs `seed`/`max`, cycles, outputs). `compile(&Checked, rate, &mut
  dyn ClipSource)` → `Arc<Program>`; `load(text, asset_id, rate, clips)` does all three.
- `Program::instantiate(seed)` → `(GraphVoice, GraphHandle)`. Install with
  `PreparedVoice::procedural(Box::new(voice), playback)`. `GraphHandle::{set, trigger}`
  take output-clock frames (non-decreasing); triggers are sample-accurate, parameter
  changes ramp over their block, values clamp to the declared range.
  `GraphHandle::activity(label)` → `capture::GraphActivity` for `Analysis::graphs`.
- `AudioGraph::{to_graph, from_graph}`, `AudioAdapter` (DomainAdapter), `projection::edge_id`.
- `commands::register`: `audiograph.text.apply`, `.rename`, `.node.add`, `.node.remove`,
  `.input.set` (node input or `output`), `.param.set`. All edits are by **name**; results
  return canonical text plus check diagnostics. The host registers the `graph` doc kind
  (`ara_graph::register`).
- `library::{Op, OpSpec, registry}`: 48 documented ops with costs (ns per block).

## Runtime (ara-runtime owner)

Compile on a loader (clips resolved through `ClipSource`), share the `Arc<Program>` per
sound, `instantiate` per voice with a seed from the deterministic gameplay RNG, and keep
the `GraphHandle` beside the `VoiceId`. Gameplay parameters/triggers go through the
handle at `engine.next_frame()` or later. Spatial voices use `output out`; stereo
graphs (`left`/`right`) need `Playback { spatial: false }`.

## Tests

`CARGO_BUILD_JOBS=2`: `cargo test -p ara-audio-graph`,
`cargo clippy -p ara-audio-graph --all-targets -- -D warnings`, `cargo fmt -p ara-audio-graph --check`.
`tests/language.rs`: canonical examples, diagnostics with line/col, reconcile identity,
projection round trip through canonical graph JSON, commands with undo.
`tests/render.rs`: sample-accurate triggers, seeds, zero allocations while rendering,
RPM/tail behaviour, every op compiles and renders, voices end on `done`, golden
captures (`tests/golden/*.json`, refresh with `ARA_UPDATE_GOLDENS=1`).
Per-voice cost: `cargo run -p ara-audio-graph --release --example graph_cost`.

## Gotchas

- Buffers are allocated in dependency order so each instruction borrows inputs and
  outputs with one `split_at_mut`; keep that invariant when changing the compiler.
- Clip values are clip-table slot indices carried in audio buffers; `switch` and
  `random.pick` are generic over audio and clips.
- Filter/EQ coefficients are recomputed every 8 samples only when modulated.
- Never compile, resolve clips or instantiate on the audio thread.
