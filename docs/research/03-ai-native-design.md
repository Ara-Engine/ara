# R03: AI-native engine design (snengine's core differentiator)

*Research track R03 · 2026-10-22 · Sources at the end ([S#] = web, [C#] = files in `.refs/` clones). Claims marked (secondary) come from trade press or vendor blogs only. Claims marked (unverified) are ones I could not confirm.*

---

## TL;DR

- **Every big engine shipped an agent surface in 2026, and each one is bolted on.** Unity has an MCP server (open beta, May 2026), a CLI with `unity command eval` (Jul 2026) and an official Claude Code plugin with 29 skills (Sep 2026) [S1,S4,S5]. Unreal 5.8 has an experimental in-process MCP plugin [S6]. Roblox Studio has an MCP server, playtest tools and a Planning Mode [S10–S12]. Blender has an official MCP server from Blender Lab (secondary) [S15]. In all of these the AI surface is a **second API** that sits beside the editor's real one. That is the gap snengine can own.
- **Benchmarks agree on where agents fail.** They fail on visual work, on graph or binary assets, on code↔asset references, and on checking runtime behavior. GameDevBench: 2D-graphics tasks pass at 33.0% vs 51.4% for gameplay, and visual feedback lifts one model from 41.1% to 52.0% [S22]. CraftBench-UE: C++ deliverables beat Blueprint ones by 30–43 points [S24]. GameLogicBench: most failed submissions run fine but behave wrong [S23]. Unity Insight: a code+asset index cuts tokens by 53% [S26].
- **Keystone decision: one typed command registry over one op-log document model.** Every edit is a typed command that produces ops on a document. Undo, redo, the editor UI, the command palette, the CLI, TS scripting, MCP, review/diff, replaying edits and (later) multiplayer editing all come from that one definition. No hand-written undo, and no separate "AI API".
- **Everything authored is canonical text with stable short IDs and dual-key references (`@path#id`).** Binary files are allowed only as *source media* (png/glb/wav) with declarative import sidecars. Derived data is cache and is never committed.
- **The engine runs as a headless daemon; the editor is one client.** The CLI, the MCP adapter and the editor UI all speak one localhost JSON-RPC "Engine Protocol". MCP is a thin generated adapter, not the core, because the MCP spec changed fundamentally again in 2026-07-28 [S16].
- **Expose about 10 MCP tools, not 300.** Use `query / inspect / schema / apply / run / capture / play / test / logs / session / docs / jobs`. The full command catalog is discovered through `sn_schema` search (UE 5.8 does the same with its 3 meta-tools [S6]). `sn_run` executes TypeScript against the typed SDK inside the current transaction ("code mode" [S19,S20]).
- **Visual feedback is a first-class API, not a screenshot hack.** It supports camera specs ("frame these entities, 3/4 view"), multi-view contact sheets, debug render modes, set-of-marks entity labels with a pixel→entity legend [S37], deterministic capture and golden-image diffs.
- **Simulation is deterministic enough to test.** It uses a fixed step, seeded RNG, ordered systems, semantic input-action injection, `step N ticks`, watch expressions, tick-level assertions and record/replay from input logs. Cross-platform bitwise determinism is out of scope.
- **Safety model: "applied-but-unreviewed".** Agent edits go live, grouped per agent turn with a "why". The editor shows them as an overlay with property diffs and before/after captures. Humans accept or revert per turn. Sessions have capability scopes. Parallel agents use git worktrees plus one daemon each, so there's no in-engine branching in v1.
- **Gameplay language: TypeScript (strict)**, with sub-second hot reload and state kept in engine-owned components, *if the architecture track can carry an embedded JS runtime*. **Make agent success rate on an internal benchmark ("snbench") a release gate from the first prototype.**

---

## 1. Landscape: how engines and tools support AI agents today (Oct 2026)

### 1.1 Engine-by-engine

| Engine / tool | Official agent surface | Mechanism | Good | Bad |
|---|---|---|---|---|
| **Unity 6 / 7α** | Unity AI Assistant (Ask/Plan/Agent modes, per-step approval in Agent mode) [S3]; **Unity MCP server** (open beta, 2026-05-11) [S1]; **Unity CLI** + experimental `com.unity.pipeline` (2026-07-20) [S4]; **official Claude Code plugin** (2026-09-09: 29 skills + CLI + MCP) [S5]; Codex plugin (2026-09-16, secondary) | In-editor "bridge" plus relay binary in `~/.unity/relay/`; custom MCP tools in C#. CLI talks to a running Editor or dev Player over a localhost API. `[CliCommand]`/`[CliArg]` attributes. `unity command eval "<C#>"` runs code live with no domain reload. `--format json`, exit codes 0/1/130 [S4] | Skills written by the teams that own each feature ("agents make reasonable guesses… which UI framework to use") [S5]. Structured CLI output. Connection approval [S1]. Generated assets carry AI-provenance metadata [S3] | MCP needs a Unity AI subscription and a Unity Cloud project [S1]. Three surfaces (MCP, CLI, Assistant), each with its own command model. The editor's internal `CommandService` is untyped (`object[] args`) and `[ExcludeFromDocs]` [C6]. YAML scenes reference by GUID+fileID |
| **Unreal 5.7 / 5.8** | 5.7 (Nov 2025): experimental in-editor AI Assistant for Q&A, guidance and C++ generation [S8]. Per a vendor test it could not place actors (secondary) [S9]. **5.8: experimental Unreal MCP plugin** [S6,S7] | MCP server inside the editor process on HTTP `127.0.0.1:8000/mcp` (no stdio/WebSocket). **Toolset Registry**: Python `unreal.ToolsetDefinition` + `@toolset_registry.tool_call`, where type hints become JSON Schema and docstrings become descriptions. C++ uses `UFUNCTION(meta=(AICallable))`. **Tool search is on by default**: `tools/list` returns only `list_toolsets`, `describe_toolset`, `call_tool` [S6] | Reflection drives the schemas. The meta-tool pattern scales. Config generator for Claude Code, Cursor, Codex and others [S6] | No approval mechanism. Calls run serially on the game thread. A new tool needs an editor restart because Live Coding won't pick up new UFUNCTIONs. Toolsets *re-implement* editor operations in Python. Telemetry is on by default [S6]. Blueprints are binary `.uasset` |
| **Roblox Studio** | Studio MCP server (toggle in Assistant) [S10]. Feb 2026 added `start_stop_play`, `run_script_in_play_mode`, `get_console_output`, `get_studio_mode`, plus BYOK (Claude/OpenAI/Gemini) [S11]. Apr 2026 added Planning Mode, agent playtesting, mesh generation on Cube, and parametric "procedural models" (coming) [S12] | Built-in MCP server and MCP client. `list_roblox_studios`/`set_active_studio` handle multi-instance [S10] | Plan → reviewable, editable action plan before execution. Play-mode scripts stop automatically and return logs [S11,S12] | Closed platform. Luau only. A text-backed DataModel exists but place files are usually binary |
| **Godot 4.8β** | No official AI integration found (unverified). Community: `Coding-Solo/godot-mcp` (CLI wrapper: launch editor, run project, read debug output) [S13]; "Godot Editor MCP" (Sep 2026: 180 typed tools, safety classes `read_only`/`mutating`+`dry_run`/`destructive`+`confirm`/`runtime`, headless GUT `run_tests`) [S13]; satelliteoflove `godot-mcp` for playtesting [S13] | Editor addon ↔ WebSocket ↔ external MCP server | The **engine itself** is the most agent-friendly here: `--headless`, `--write-movie` + `--fixed-fps` (deterministic offline frames), `--quit-after`, `--doctool`, `--dump-extension-api-with-docs` [C1]; text `.tscn` that now carries `unique_id`/`parent_id_path` per node so it survives renames [C2]; 833 XML class docs [C5] | Undo is hand-written do/undo method pairs [C3]. Configuration warnings are plain strings with no codes or fixes [C4]. 180 tools is a lot of context |
| **Blender** | Official **Blender Lab MCP** (2026, Blender 5.1+, warns it "runs LLM-generated code without guards", secondary/partly unverified) [S15]. Community `blender-mcp` (ahujasid) [S15] | Add-on socket ↔ MCP server. Community tools seen in this session's tool list: `get_scene_info`, `get_object_info`, `get_viewport_screenshot`, `execute_blender_code`, plus Hyper3D/Hunyuan3D/PolyHaven/Sketchfab sourcing | Small tool set + `exec` escape hatch + viewport screenshot is a very effective combination | `exec` has no transaction boundary, so review is impossible. Safety is "use a VM" |
| **Bevy** | **Bevy Remote Protocol (BRP)**, JSON-RPC 2.0 built into the engine (`RemotePlugin`) [S14]. `bevy_brp_mcp` (now in `natepiano/bevy_brp`) | `world_query`, `world_mutate_components`, `world_get_components_watch`, `brp_type_guide` (mutation paths + JSON shapes). Extras add screenshots (window / camera / **crop around an entity**), `send_keys`, mouse, gamepad [S14] | A protocol first, MCP second: the right layering. A type guide gives exact JSON shapes | `rpc_discover` returns method names without schemas (per README) [S14]. Runtime-only, no editor |
| **UGC platforms** | **Meta Horizon Studio / Horizon Engine**: agentic assistant that generates meshes, textures, skyboxes, audio and **TypeScript** (2025 alpha; 2026 Horizon Create, secondary) [S33]. **UEFN**: Epic Developer Assistant for Verse; "Conversations" AI NPCs (Gemini + ElevenLabs, v40.20, Apr 2026) [S34]. **Rec Room**: Maker AI (experimental, spatially aware, applies behaviors, date unverified) [S36]. Core: not researched | Platform-hosted assistants | Narrow, opinionated APIs make agents more reliable | Closed. Agents can't be brought in from outside |

### 1.2 Third-party agents and AI-native startups

- **Engine copilots.** Ramen's **Aura** (Unreal + Unity) launched Jan 2026. Its "Telos" Blueprint agent claims a 25× drop in graph-interpretation errors. Aura added verification and level-design agents and bought **Coplay** (Unity, Mar 2026) and Flopperam (Jun 2026) [S28]. **Bezi** (Unity) indexes the whole project and offers Agent Mode with **checkpoints/restore** [S29]. Several commercial Unreal MCPs advertise 300–400+ tools (vendor claims) [S9].
- **AI-native "engines".** Moonlake AI ($28M seed) has a "world modeling agent" for its *Generative Game Engine* in open beta [S30]. Astrocade (text-to-game, $56M), Nilo (browser AI-native 3D, $4M), SPARQ ($8.5M) and Atelico ($5M) are all secondary-sourced [S31]. Engine/creation platforms were a small slice of 2026 gaming VC ($71M over 5 deals vs Meshy's $400M Series B) [S31]. **World models** (Google Project Genie on Genie 3, Jan 2026) are explicitly "not a game engine" [S32]. They are complements for ideation, not competitors.
- **Agent playtesting.** nunu.ai (vision + keyboard/mouse agents) and modl.ai (black-box, vision-only) [S35]. Both work *around* engines because engines expose no semantic game state.

### 1.3 Agent-platform patterns snengine should build on

- **MCP 2026-07-28** is now **stateless**. There's no `initialize` handshake and no sessions; state lives in "explicit, server-minted handles passed as ordinary tool arguments". **Tasks** became an extension (`tasks/get` polling). Sampling, Roots and Logging are deprecated. `tools/list` **SHOULD** have a deterministic order (helps prompt caching). Schemas accept full JSON Schema 2020-12 [S16]. **MCP Apps** (`ui://` HTML resources in a sandboxed iframe) went stable in Jan 2026 [S17]. A review/diff UI could therefore render *inside* the agent client.
- **Tool overload is a known, solved-ish problem.** Anthropic's tool search improved MCP-eval accuracy from 49%→74% (Opus 4) and 79.5%→88.1% (Opus 4.5) [S18]. "Code execution with MCP" cut one workflow from 150k to 2k tokens [S19]. Cloudflare's Code Mode argues LLMs are better at writing TS against a typed API than at emitting tool calls [S20]. Claude Code itself (this session) defers MCP tools behind a `ToolSearch` tool.
- **Skills / AGENTS.md / llms.txt.** Unity ships engine-authored skills [S5]. Flax's repo ships an `AGENTS.md` [C9]. The MCP docs publish `llms.txt` [S16,S38]. **ACP** (Agent Client Protocol, Zed + JetBrains) lets an editor host *any* coding agent, with ~40 clients claimed (secondary) [S21].

### 1.4 What coding agents (Claude Code, Codex, Cursor) actually do well and badly

**Well:** editing text code (C#, C++, GDScript, TS) and text scenes such as `.tscn`. Driving CLIs with structured output (Unity's demo: bug report → `command eval` → fix disabled collider → re-enter play) [S4]. Following engine-authored skills [S5]. Building a playable foundation from scratch [S25].

**Badly:**

- Binary or graph assets: Blueprint lags C++ by 30–43 pp. 42–50% of Blueprints that pass asset checks still fail runtime assertions [S24].
- Visual and 2D-graphics tasks [S22].
- Checking that runtime behavior is *correct*, not just that it compiles [S23,S25].
- Finding defects and keeping existing functionality intact [S25].
- Cross-file code↔asset questions through GUIDs [S26].
- Picking among overlapping legacy systems [S5].
- Under open network access, copying code from public repos [S23].

---

## 2. Where agents struggle with today's engines

| Pain point | Evidence today | snengine answer |
|---|---|---|
| **Binary/opaque assets** | Blueprints in `.uasset` [S24]. Flax `Content/` is 151 binary `.flax` packages; only scenes are JSON [C9] | All authored data is canonical text, graphs included. Binary only for source media |
| **GUI-only workflows** | UE 5.7 Assistant couldn't place actors [S9]. Bake, import and project settings live behind dialogs | Every action is a registered command; the UI is just one client of it |
| **Hidden editor state** | Unsaved scene state, selection, play state, domain-reload state. External file edits fight the open editor (practitioner reports, unverified) | "Always-saved" documents. Editor state (selection, open scene, camera, play state) is itself queryable |
| **Slow iteration** | UE: new tools need a restart [S6]. Unity: domain reloads (Unity 7 is moving to CoreCLR, secondary [S4]) | Hot reload under 1 s; live command registry refresh |
| **No/slow headless** | Unity: old Hub headless path "took noticeably longer" [S4]. Godot does this right [C1] | Daemon-first architecture |
| **No visual feedback** | +10.9 pp from visual feedback [S22] | `sn_capture` with marks, modes, multi-view, goldens |
| **Unstructured logs** | Godot warnings are `PackedStringArray` [C4]. Console text everywhere | Diagnostics with stable codes, locations and machine-applicable fixes |
| **Nondeterminism** | "Runnable but wrong" is the dominant failure [S23] | Fixed step, seeded RNG, tick assertions, replay |
| **Huge APIs, stale docs, many ways** | Unity skills exist partly because of uGUI vs UITK vs IMGUI choice [S5] | One way per job. Docs, types and skills generated per engine version |
| **GUID/reference hell** | Unity Insight [S26]. Godot had to add `unique_id`/`parent_id_path` [C2] | Dual-key refs `@path#id`, live reference index, `refs.rename` command |
| **Merge conflicts** | Unity YAML needs a smart-merge tool. UE added One-File-Per-Actor | Flat entity tables, canonical formatter, optional per-entity files, semantic merge driver |
| **Tool sprawl** | 180 / 305 / 400+ tool servers [S13,S9] | About 10 tools + schema search + code mode |
| **Unsafe autonomy** | UE MCP has no approval [S6]. Blender Lab: "runs LLM-generated code without guards" [S15] | Sessions, scopes, per-turn transactions, review overlay |

---

## 3. Design principles: evaluation

**P1. Everything is text. AGREE, with refinements.**
1. *Authored* data is text (scenes, prefabs, materials, shader/VFX/anim/logic graphs, UI, input maps, project settings).
2. *Source media* (textures, meshes, audio) stays binary, with a text import sidecar holding the ID and import settings.
3. *Derived* data (cooked meshes, shader binaries, lightmaps) lives in a content-addressed cache and is never committed.
4. "Text" alone isn't enough. Unity YAML is text and still hostile. The format also has to be **canonical** (one serialization per value), **flat** (an entity table, not deep nesting, so a reparent is a one-line diff), **defaults-omitted**, **named** (refs carry readable paths), and **schema-validated** (`$schema` gives VS Code and agents completion for free).

**P2. Every editor action is one typed command. AGREE. This is the keystone.**
Refinement: separate *commands* (edits to the document that produce ops), *queries* (read-only) and *jobs* (long-running side effects such as import, bake, build or generate). Undo is computed generically from ops, never hand-written. Compare Godot's manual do/undo pairs [C3], Unity's split MenuItem/CommandService/Undo/SerializedObject [C6,C7], and O3DE's ActionManager plus 298 files of `ScopeFlags::Automation` reflection feeding Python `azlmbr` [C8]. O3DE is the closest precedent. It still keeps the action UI and the scripting API as two systems.

**P3. Reflection-generated schemas + live introspection. AGREE.**
Add *semantic* metadata: units (`m`, `deg`, `s`, `kg`), ranges, enums, defaults, docs, examples, `since` and `deprecated → replacement`. Godot's docs warn that angles are radians [C5], which is a classic LLM error that a unit in the schema prevents. Generate JSON Schema, `.d.ts`, docs pages, inspector UI and MCP catalogs from one source.

**P4. Headless + fast boot + CLI for everything. AGREE.**
Go further and make the engine a **daemon**. The GUI editor is a client, like an LSP server and its editors. Budgets: warm headless project open under 2 s, CLI round-trip under 100 ms, capture under 500 ms, hot reload under 1 s.

**P5. Visual feedback. AGREE, high priority.** Refinement: images must come with a **legend** (mark → entity ID, bbox) so the model can act on what it sees. Captures must be deterministic (no TAA jitter or auto-exposure drift; settle N frames) so diffs mean something.

**P6. Determinism, replay, time control, automated playtesting. AGREE, scoped.**
Target same-binary, same-platform determinism. Cross-platform bitwise determinism costs too much: Jolt, for example, has to disable FMA under `JPH_CROSS_PLATFORM_DETERMINISTIC` [C10]. Inject input at the **action** level ("Jump", "Move(1,0)"), not as raw keys, so agent tests survive rebinding and stay readable.

**P7. Structured logs/errors with fix hints + LLM-friendly docs. AGREE.**
Fixes must be *machine-applicable commands*, like rustc suggestions or TS code actions, not prose.

**P8. Safety: transactions, sandboxes, preview/diff/approve, permissions, world branching. AGREE, simplified for v1.**
Preview/dry-run, per-turn transactions, checkpoints, scopes and a review overlay all go in v1. *In-engine world branching* does not; git worktrees with one daemon per worktree give real isolation for free because everything is text. Treat in-engine branching as v2 only if users ask.

**P9. Gameplay language AI writes well, with fast hot reload. AGREE.** See §4.11. Recommendation: TypeScript.

**P10. In-engine AI features (asset gen, NPC AI), optional and provider-agnostic. AGREE, low priority.** The differentiator is the *agent interface*, not first-party models. Models and vendors churn monthly (Meshy, Hunyuan3D, Rodin, ElevenLabs…). Build slots, not models.

**Added principles**

- **P11. One way to do each thing.** Never ship two UI systems, two input systems or two render pipelines side by side. Deprecate hard and ship codemods. Agent priors from mixed corpora are the enemy.
- **P12. Files and commands are both first-class.** Coding agents are best at editing files, and live editors are best at commands. Both paths must work. A file save triggers validate → hot-reload → diagnostics. A command produces ops → debounced flush to the canonical file.
- **P13. Agents are users.** There is no agent-only API. Anything an agent can do appears in the command palette, and anything a human does is logged as command calls the agent can read ("what did I just do?").
- **P14. Budgeted, paginated, handle-based outputs.** Every query takes `limit`/`detail`/`fields`, returns handles, and never dumps a 10k-entity scene into context.
- **P15. Measure it.** Agent success rate on a fixed task suite is a product metric and a release gate.

---

## 4. Recommendations for snengine

### 4.1 Process architecture

```
            ┌──────────── snd (engine daemon, one per project/worktree) ─────────────┐
            │ Document store (op log) · Command/Query/Job registry · Reflection       │
            │ Asset index + ref graph · Sim (edit world / play worlds) · Renderer     │
            │ Diagnostics bus · Session manager (scopes, txns, checkpoints)           │
            └───────────────▲─────────────────▲───────────────────▲──────────────────┘
                 Engine Protocol (JSON-RPC 2.0; WebSocket + stdio; localhost + token)
          ┌──────────────┐  │  ┌───────────┐  │  ┌────────────────┐ │  ┌──────────────┐
          │ Editor UI    │──┘  │ sn CLI    │──┘  │ MCP adapter    │─┘  │ TS scripts / │
          │ (a client)   │     │ (thin)    │     │ (generated)    │    │ tests (SDK)  │
          └──────────────┘     └───────────┘     └────────────────┘    └──────────────┘
```

- **Engine Protocol (EP)** is the superset. It has streaming subscriptions for the editor UI, binary frames for viewport/capture transport, and batching. Bevy's BRP shows the right layering [S14].
- **MCP adapter** is generated from the registries and serves Streamable HTTP plus a stdio shim. It follows 2026-07-28 statelessness: every call carries a `session` handle as an argument [S16]. Long jobs map to the tasks extension. Tool order is deterministic.
- **One daemon per worktree.** Parallel agents work in separate git worktrees. Isolation comes free from text files.
- **ACP later.** An in-editor "Agents" panel can host Claude Code, Codex or others over ACP [S21], so the editor never builds its own chat agent and doesn't chase model churn.

### 4.2 Document model and ops (the keystone)

All authored state is a set of **documents** (scene, prefab, material, graph, UI, settings). Each is a table of typed records addressed by `(docId, recordId, fieldPath)`. Every mutation is an op:

```ts
type FieldPath = string;            // "Transform.position.x", "children[3]" — like Unity's SerializedProperty.propertyPath [C7]
type Op =
  | { op: "create"; doc: DocId; id: RecordId; type: string; value: Json; parent?: RecordId; index?: number }
  | { op: "set";    doc: DocId; id: RecordId; path: FieldPath; value: Json; prev: Json }       // prev ⇒ free inverse
  | { op: "delete"; doc: DocId; id: RecordId; snapshot: Json }                                 // snapshot ⇒ free inverse
  | { op: "move";   doc: DocId; id: RecordId; parent: RecordId | null; index?: number; prevParent: RecordId | null; prevIndex?: number };

interface Txn {                       // one undo step = one agent turn or one human gesture
  id: TxnId; session: SessionId; author: "human" | `agent:${string}`;
  why?: string;                       // agent-supplied rationale, shown in review
  commands: CommandCall[];            // what was asked (replayable, human-readable)
  ops: Op[];                          // what changed (invertible, diffable, mergeable)
  status: "applied" | "accepted" | "reverted";
}
```

What falls out of this for free:

- **Undo/redo:** apply the inverse ops.
- **Review diffs:** the ops grouped by record.
- **Agent "what changed since X":** the op log since a cursor.
- **Macro recording:** commands become a TS script.
- **Live sync to the file:** debounced canonical write.
- **Later, collaborative editing:** ops are already the CRDT/OT unit.

Non-document state (selection, viewport camera, open panels, play state) lives in an **editor-state document** that's queryable but not committed.

### 4.3 Command registry: defined once, exposed everywhere

Shape sketch (TS shown; native core commands register the same metadata through the reflection macros):

```ts
import { command, t } from "@snengine/editor";

/** Set position/rotation/scale of entities. Used by gizmos, inspector, CLI, scripts and agents. */
export const setTransform = command("transform.set", {
  summary: "Set position, rotation and/or scale of one or more entities.",
  params: t.object({
    entities: t.array(t.entityRef).min(1),
    position: t.vec3.unit("m").optional(),
    rotation: t.euler.unit("deg").optional(),       // units are part of the schema, not the docs
    scale:    t.vec3.optional(),
    space:    t.enum("local", "world").default("local"),
  }),
  kind: "edit",                                     // "query" | "edit" | "runtime" | "job"
  hints: { idempotent: true, destructive: false },  // → MCP tool annotations, approval policy
  scope: "edit:scene",                              // capability required
  examples: [{ entities: ["e_8b21"], position: [2, 0, 3] }],
  ui: { palette: true, menu: "Edit/Transform/Set…" },
  run(ctx, p) {
    for (const e of p.entities) {
      if (p.position) ctx.set(e, "Transform.position", ctx.toSpace(e, p.position, p.space));
      if (p.rotation) ctx.set(e, "Transform.rotation", ctx.toSpace(e, p.rotation, p.space));
      if (p.scale)    ctx.set(e, "Transform.scale", p.scale);
    }
  },
});
```

From this single definition the registry generates:

- JSON Schema and catalog entry (MCP / `sn_schema`)
- typed SDK `sn.transform.set({...})` and `.d.ts`
- CLI `sn do transform.set --entities e_8b21 --position 2,0,3`
- command-palette and menu entries
- an inspector "Run…" form
- a docs page, examples and the `llms.txt` entry

`run` only touches the document through `ctx`, so ops, undo and review are automatic. **Plugins and game code register commands the same way.** There is no second API (unlike UE's Python toolsets [S6]).

### 4.4 Text formats

Scene sketch (canonical JSON with comments + trailing commas allowed; the formatter owns key order and number formatting):

```jsonc
// levels/arena.scene  — written by `sn fmt`; never hand-ordered
{
  "$schema": "../.sn/schema/scene.schema.json",
  "format": "sn.scene/1",
  "id": "scn_7kq2m9",
  "entities": {                                   // FLAT table: reparent = 1-line diff
    "e_1c0d": { "name": "Floor",
                "Transform": { "scale": [40, 0.5, 40] },
                "MeshRenderer": { "mesh": "@meshes/cube.glb#m_box", "material": "@materials/concrete.mat#mt_2a" },
                "Collider": { "shape": "box" } },
    "e_4f9a": { "name": "Player", "prefab": "@prefabs/player.prefab#pf_2x81",
                "Transform": { "position": [0, 1, 0] },
                "overrides": { "Health.max": 150 } },
    "e_8b21": { "name": "Crate", "parent": "e_1c0d", "order": 0,
                "Transform": { "position": [2, 0, 3], "rotation": [0, 45, 0] } }   // rotation in degrees, per schema
  }
}
```

Rules:

- **IDs are short, prefixed and random** (`e_`, `pf_`, `mt_`, base32, unique per doc; assets unique per project). They're not 128-bit GUIDs, which LLMs mangle.
- **References are dual-key: `@path#id`.** The path is for humans and LLMs; the ID is for robustness. If the path breaks, the loader resolves by ID and `sn fmt` rewrites the path. If the ID is missing (an agent wrote only a path), it resolves by path and fills in the ID. Self-healing either way.
- In batch commands, **agents may mint symbolic IDs** (`"$crate1"`). The engine returns the mapping, as Figma/Linear-style client IDs do.
- **Large levels:** a scene folder with one file per entity or cell (the UE One-File-Per-Actor idea) behind the same document abstraction.
- **Graphs** (material, VFX, animation, logic) use the same flat record table for nodes and edges. The agent-facing canonical form is text. Make sure a graph can also be *authored as an expression/DSL* where natural (shader math), because text code beats graphs for agents by about 30–40 pp [S24].
- **Git:** ship a `sn merge` semantic merge driver (record-level three-way merge on the flat table) and `.gitattributes` templates.

### 4.5 The agent interface (MCP view of the Engine Protocol)

**Twelve tools, each a thin view over the registries:**

| Tool | Purpose | Notes |
|---|---|---|
| `sn_query` | Find entities, assets, refs, editor state | Compact query syntax plus JSON form. Paginated; returns handles. Precedent: Unity QuickSearch `QueryEngine` [C7] |
| `sn_inspect` | Full typed state of entities, assets or components | `fields`, `depth`, `detail: "summary"\|"full"`. Includes `schemaRef` |
| `sn_schema` | Search and describe components, commands, queries, jobs and enums | This is the tool-search layer (UE's `list_toolsets`/`describe_toolset` [S6]) |
| `sn_apply` | Run a batch of commands atomically in the session | `dryRun` returns a diff without applying; `why`; symbolic IDs |
| `sn_run` | Execute TS against the typed SDK in a sandboxed isolate | Code mode [S19,S20]. **All mutations go through ops, so `run` is still reviewable**. No fs/net unless scoped |
| `sn_capture` | Render images | Camera specs, modes, marks + legend, multi-view, golden compare |
| `sn_play` | Play-world control | start/stop/pause/step/timescale, action input, watch, until, record/replay, snapshot/restore |
| `sn_test` | Run unit, play and golden tests | Structured results; failures carry captures and replay handles |
| `sn_logs` | Diagnostics stream | Cursor-based (`since`), filter by code/severity/entity, de-duplicated with counts |
| `sn_session` | begin/status/diff/checkpoint/rollback/submit | Scopes; returns the `session` handle that every other tool takes |
| `sn_docs` | Versioned docs, guides and examples search | Same content as `llms.txt` and the skills |
| `sn_jobs` | import/bake/build/generate | Long-running; MCP tasks extension [S16] |

Example: batch apply with symbolic IDs, then a failure with a machine-applicable fix:

```jsonc
// → sn_apply
{ "session": "ses_9f2", "why": "Stack two crates near spawn",
  "commands": [
    { "cmd": "entity.instantiate", "as": "$c1", "args": { "prefab": "@prefabs/crate.prefab", "parent": "e_1c0d", "position": [2, 0.5, 3] } },
    { "cmd": "entity.instantiate", "as": "$c2", "args": { "prefab": "@prefabs/crate.prefab", "parent": "$c1", "position": [0, 1, 0] } },
    { "cmd": "transform.set", "args": { "entities": ["$c2"], "rotation": [0, 15, 0] } } ] }

// ← failure (nothing applied; transaction is atomic)
{ "ok": false, "failedAt": 0,
  "diagnostics": [{
    "code": "SN2104", "severity": "error",
    "message": "Asset '@prefabs/crate.prefab' not found.",
    "where": { "command": 0, "param": "prefab" },
    "fixes": [{ "title": "Use '@props/crate.prefab' (closest match, same type)",
                "edit": { "commands[0].args.prefab": "@props/crate.prefab", "commands[1].args.prefab": "@props/crate.prefab" } }],
    "docs": "sn://docs/errors/SN2104" }] }

// ← success
{ "ok": true, "txn": "tx_41", "ids": { "$c1": "e_k2p8", "$c2": "e_k2p9" },
  "diff": { "created": ["e_k2p8", "e_k2p9"], "modified": [], "deleted": [] }, "diagnostics": [] }
```

**Scene-to-text** (the default `sn_inspect`/`sn_query` summary format, tuned for tokens and spatial reasoning):

```
scene arena (scn_7kq2m9) · 142 entities · showing depth≤2, 6 of 142 · units m/deg · Y-up
e_1c0d Floor        [MeshRenderer Collider]      pos(0,0,0)   size 40×0.5×40
  e_k2p8 Crate      [MeshRenderer RigidBody]     pos(2,0.5,3) on e_1c0d · 3.6m from Player
    e_k2p9 Crate    [MeshRenderer RigidBody]     pos(0,1,0)   rot(0,15,0)
e_4f9a Player       prefab:player [Controller Health=150/150]  pos(0,1,0)
e_77c1 MainCamera   [Camera fov=60]               looking at e_4f9a
… 136 more · next: sn_query {"cursor":"q_3"}
```

**How it avoids dumping thousands of tools on the model:**

1. A fixed set of 12 tools.
2. Commands are namespaced (`entity.*`, `transform.*`, `material.*`, `ui.*`, `anim.*`, `nav.*`, `asset.*`, `project.*`, `game.*` for user code) and found with `sn_schema {search}`.
3. For anything multi-step, `sn_run` with the full typed SDK `.d.ts`, which the model can read on demand as files, per Anthropic's progressive disclosure [S19].
4. Generated **skills** (task recipes) and `AGENTS.md` cover project conventions.

### 4.6 Visual feedback API

```jsonc
// → sn_capture
{ "session": "ses_9f2",
  "views": [
    { "camera": { "frame": ["e_k2p8", "e_k2p9"], "angle": "three-quarter", "padding": 0.2 } },
    { "camera": { "ortho": "top", "frame": "scene" } },
    { "camera": "e_77c1" }                                   // an existing camera, e.g. gameplay view
  ],
  "size": [768, 512],
  "mode": "lit",            // lit | unlit | wireframe | normals | overdraw | collision | navmesh | lightmap-uv | ui-bounds
  "marks": { "entities": "visible", "max": 40, "style": "numbered" },   // set-of-marks [S37]
  "overlays": ["grid-1m", "bounds:selected", "axes"],
  "deterministic": true,    // fixed time, no TAA jitter, locked exposure, settle 8 frames
  "compare": { "golden": "tests/goldens/crate-stack.png", "metric": "flip", "tolerance": 0.02 } }

// ← image(s) + legend
{ "images": ["sn://captures/cap_88/0.png", "sn://captures/cap_88/1.png", "sn://captures/cap_88/2.png"],
  "legend": [{ "mark": 1, "entity": "e_k2p8", "name": "Crate", "bbox": [312, 220, 88, 90], "view": 0 }, …],
  "compare": { "score": 0.031, "pass": false, "diffImage": "sn://captures/cap_88/diff.png" } }
```

Also needed:

- `capture.ui` renders a UI document in isolation at several resolutions/DPIs and returns computed layout rects as text.
- `capture.sequence` returns N frames or a short clip from a play world for motion bugs (GameDevBench uses video feedback too [S22]).
- The **pixel → entity** lookup (ID buffer) also powers "click in screenshot = select" for humans.

### 4.7 Play, test, replay

```jsonc
// → sn_play
{ "session": "ses_9f2", "action": "start", "scene": "@levels/arena.scene", "seed": 7, "fixedDt": 0.0166667, "record": true }
{ "action": "step", "ticks": 120,
  "input": [ { "tick": 0, "action": "Move", "value": [1, 0], "holdTicks": 60 }, { "tick": 45, "action": "Jump" } ],
  "watch": ["e_4f9a.Transform.position.y", "e_4f9a.Health.current"], "sampleEvery": 15,
  "until": "e_4f9a.Grounded.value == false" }
// ← { "tick": 46, "stoppedBy": "until", "samples": {…}, "events": [{ "tick": 45, "type": "Jumped", "entity": "e_4f9a" }],
//     "diagnostics": [], "replay": "rep_19" }
```

Playtests are TS files run headless by `sn test` and `sn_test`:

```ts
playtest("player can jump onto crate", async (t) => {
  const w = await t.load("@levels/arena.scene", { seed: 1 });
  const player = w.find("Player");
  await t.input.hold("Move", [1, 0], { ticks: 40 });
  await t.input.press("Jump");
  await t.until(() => player.get(Grounded).value, { maxTicks: 180 });
  t.expect(player.get(Transform).position.y).toBeGreaterThan(0.9);   // tick-level state assertion [S23]
  await t.capture("after-jump", { camera: { follow: player }, golden: true });
});
```

Requirements:

- **Replay**: `replay = scene hash + seed + input log + build id`. Periodic **snapshots** allow scrubbing.
- **High-level drivers** for exploratory agents: `agent.goto(entity)` over the navmesh, `agent.interact(entity)`.
- **Semantic game state**: anything tagged `@observable` in game code is queryable during play. Black-box vision QA (nunu/modl [S35]) becomes the fallback rather than the only option.

### 4.8 Diagnostics

```ts
interface Diagnostic {
  code: `SN${number}` | `${string}/${number}`;         // engine codes + namespaced plugin/game codes
  severity: "error" | "warning" | "info";
  message: string;
  where?: { file?: string; line?: number; col?: number; doc?: DocId; entity?: RecordId; asset?: AssetId; path?: FieldPath; command?: number };
  cause?: Diagnostic[];
  fixes?: { title: string; commands?: CommandCall[]; edit?: Record<string, Json> }[];   // machine-applicable
  docs?: `sn://docs/errors/${string}`;
  count?: number; firstTick?: number; lastTick?: number;                               // de-duplication
}
```

- **One bus** carries script exceptions (source-mapped TS stacks), schema validation, broken refs, import errors, shader compile errors (with node IDs for graph shaders) and runtime asserts.
- **`sn check`** = typecheck + schema validate + ref check + "doctor" rules. Example: "entity has MeshRenderer but no material" → fix: `material.assign default`. This is a typed, fixable version of Godot's configuration warnings [C4].
- **`sn why <entity>`** runs targeted doctor rules ("why isn't this visible?" checks layers, culling, material, camera far plane and disabled state).

### 4.9 Docs and skills

Generate per engine version from reflection and the registries:

- `llms.txt` / `llms-full.txt` [S38]
- per-type markdown pages
- `.d.ts`
- compiled-and-tested examples. Doc examples run in CI; anything that fails to compile blocks release.

`sn init` writes `AGENTS.md` (conventions: Y-up, meters, degrees, ID/ref syntax, "use commands, not raw file surgery, for graphs") plus engine **skills** (Unity precedent [S5]). `sn skills sync` regenerates both on engine upgrade. Project-specific components and commands get folded into a project-level `llms.txt`.

### 4.10 Safety, permissions and human review

**Sessions.** `sn_session begin { client, scopes, label }`. Scopes are: `read`, `edit:scene`, `edit:assets`, `edit:code`, `play`, `run` (sandboxed TS), `import`, `bake`, `build`, `generate:$` (spends money), `fs:outside-project`, `net`.

- **Default for MCP clients:** everything except `build:publish`, `fs:outside-project`, `net` and `generate:$`.
- **Connections:** first connection requires approval in the editor (Unity precedent [S1]). Localhost only, with a per-project token.

**Transactions.** Each `sn_apply`/`sn_run` call is one Txn carrying `why`. A checkpoint is created automatically at each agent turn boundary. `rollback {to}` reverts by inverse ops and detects conflicts when a human edited the same path since.

**Applied-but-unreviewed (recommended v1 UX).** Agent changes apply live, the same way Claude Code and Cursor apply edits and then let you keep or undo them. The editor then:

- **Outlines** changed entities in the viewport and hierarchy: green = added, amber = modified, red ghost = deleted.
- **Inspector:** shows `was → now` per field.
- **Agents panel:** a timeline of Txns with `why`, the commands, and before/after capture sliders. Accept and Revert work per Txn or per entity.
- **Agent presence:** the agent's selection and camera appear like a collaborator's (Figma-style). "Follow agent" mirrors its captures in a viewport tab.
- **Code changes:** text files show as normal diffs. Because everything is text, `git diff` is always the ground truth.

**Approval policy.** Commands with `hints.destructive` (delete asset, overwrite source media, `fs` writes) need a confirm unless the session's policy allows them. `dryRun` is always available.

**`run`/eval safety.** Unlike Blender's `exec` [S15] or Unity's `command eval` [S4], `sn_run` executes in an isolate whose only write path is the op log. Eval'd code is therefore just as reviewable and revertible as commands.

**Prompt-injection hygiene.** Tool results that include user or marketplace text (asset descriptions, package READMEs) are wrapped as `untrusted` content with a provenance field.

**Parallel agents.** One git worktree and one daemon each; merge via git + `sn merge`. In-engine world branching is deferred.

### 4.11 Gameplay language

| Criterion | **TypeScript** | C# | Luau | Python |
|---|---|---|---|---|
| LLM proficiency / corpus | Top tier; #1 on GitHub by contributors since Aug 2025, a rise GitHub links to agents [S27] | Top tier, but the corpus is **Unity-flavored**, so agents hallucinate Unity APIs in a non-Unity engine | Good, smaller corpus (Roblox) | Top tier |
| Types catch agent errors | Strong (a cited study found 94% of LLM compile errors are type-check failures, secondary [S27]) | Strong | Gradual | Weak at runtime |
| Hot reload | Excellent (transpile in ms; module swap) | Decent (Unity's reloads are the pain point) | Excellent | Excellent |
| Performance | V8 JIT good. **JIT-banned consoles/iOS need an interpreter/AOT** (QuickJS/Hermes-class) | Excellent (CoreCLR/NativeAOT) | Good interpreter | Poor |
| Sandboxing for `sn_run` | Excellent (isolates) | Hard | Excellent | Hard |
| Owner fit / web reuse (UI, tooling, schemas) | Perfect (TS dev; schemas → `.d.ts`; React-like UI priors) | Good (Unity dev) | Low | Low |

**Recommendation:** strict TypeScript for gameplay, editor extensions and commands.

- Types come from reflection (`sn types`).
- Game state lives in engine-owned components/resources, never in module closures, so hot reload is a module swap with zero state migration.
- Systems are functions over queries.
- Performance-critical code stays native in the core.
- Meta Horizon already uses TS for creator scripting [S33].

This is conditional on the architecture track accepting an embedded JS runtime plus a console story. If not, C# on CoreCLR is the fallback, with explicit "not Unity" skills and lint rules to fight Unity priors.

### 4.12 In-engine AI features (optional, provider-agnostic)

- **Generation as jobs:** `generate.mesh|texture|material|audio|voice|anim` through a `GenerationProvider` interface (keys in the OS keychain, cost estimate before running, `generate:$` scope).
- **Outputs** are ordinary source assets plus a sidecar with **provenance** (provider, model, prompt, seed, date). Unity tags generated assets the same way [S3], and storefront AI disclosure requires it.
- **Runtime AI (NPC dialog, voice)** ships as a plugin with the same provider abstraction (UEFN uses Gemini + ElevenLabs [S34]).
- **Parametric generators:** TS functions that output document records (Roblox's "procedural models" idea [S12]). Agents and humans can then re-run them with new parameters, unlike baked meshes.
- **Ranking:** none of this is on the critical path. The agent interface is.

### 4.13 snbench: measure the differentiator

Start from the first prototype with 50–100 tasks modeled on GameDevBench, GameLogicBench and CraftBench-UE [S22–S24]:

- scene assembly
- material/VFX tweaks
- UI screens at 3 resolutions
- gameplay mechanics with tick assertions
- bug-fix tasks with injected defects (GameXpert-style [S25])

Run them nightly with 2–3 frontier agents over MCP. Track pass rate, tokens, wall time and human-review reverts. **A regression in snbench is a release blocker**, the same as a perf regression.

---

## 5. Top 10 AI-native features (ranked by impact ÷ effort)

| # | Feature | Impact | Effort | Why this rank |
|---|---|---|---|---|
| 1 | **Typed command/query/job registry over an op-log document model** (undo, UI, CLI, MCP, review from one definition) | ★★★★★ | M (but day-1 only; retrofitting is near impossible) | Everything else depends on it. Eliminates the "second API" every incumbent has |
| 2 | **Canonical text formats + short stable IDs + `@path#id` refs + live ref index + `sn fmt`/`sn merge`** | ★★★★★ | M | Directly removes the binary, GUID and merge pains [S24,S26] |
| 3 | **Reflection → JSON Schema / `.d.ts` / docs / `llms.txt` / skills, with units and semantics** | ★★★★ | S | Cheap once reflection exists; kills hallucinated APIs and unit errors |
| 4 | **Daemon + Engine Protocol + generated 12-tool MCP adapter with schema search and `sn_run` code mode** | ★★★★★ | S–M | The actual product surface for agents; small because it's generated |
| 5 | **Structured diagnostics with codes + machine-applicable fixes + `sn check` / `sn why`** | ★★★★ | S–M | Closes the edit → verify loop in seconds |
| 6 | **`sn_capture`: camera specs, debug modes, set-of-marks legend, deterministic capture, golden diffs** | ★★★★ | M | +10.9 pp measured from visual feedback alone [S22] |
| 7 | **Review UX: sessions, scopes, per-turn Txns, applied-but-unreviewed overlay, before/after, rollback** | ★★★★ | M | Makes autonomy trustworthy for humans; Pillar 3 + 4 synergy |
| 8 | **Deterministic play control: fixed step, seeded RNG, action-level input injection, watch/until, tick assertions, record/replay** | ★★★★★ | M–L | Attacks the #1 failure mode, "runs but wrong" [S23,S25] |
| 9 | **TS gameplay with sub-second hot reload, engine-owned state, sandboxed isolates** | ★★★★ | L | Big bet; also gives `sn_run` its sandbox and UI its React-like priors |
| 10 | **Provider-agnostic generation jobs with provenance + parametric generators** | ★★★ | M | Nice demo value; commodity models; keep it a plugin |

Also: **snbench** (§4.13) is not a feature, but it's what keeps 1–10 honest.

---

## 6. Architectural requirements imposed on the core (for the architecture, editor and UI tracks)

1. **Single document model** for *all* authored data (scenes, prefabs, materials, graphs, UI, input maps, settings): typed records addressable by `(doc, id, fieldPath)`. **All** mutations are invertible ops. There are no side-channel writes, including from the editor UI and plugins.
2. **Reflection is the single source of truth**, carrying semantic metadata (units, ranges, enums, defaults, docs, examples, since/deprecated). It's exportable to JSON Schema 2020-12 and `.d.ts` at runtime, with no restart to pick up new types or commands (UE needs one [S6]).
3. **Command/Query/Job registries** that core, plugins and game code all register into. Each entry has a params schema, `kind`, `hints` (idempotent/destructive), required `scope` and optional UI placement.
4. **Daemon-first process model**: the engine core runs headless with no window. The editor UI is a protocol client. The protocol supports subscriptions (op stream, diagnostics, editor state) and binary payloads.
5. **Offscreen/headless rendering** on GPU, plus a CI fallback (software rasterizer) for captures. Needs an **entity ID buffer**, debug view modes, overlay pass, a deterministic capture mode (no temporal jitter, fixed exposure, settle frames), and multi-camera render-to-texture on demand.
6. **Simulation**: fixed-timestep mode, deterministic system ordering, a central seeded RNG service, injectable clock, faster-than-real-time stepping, world snapshot/restore, and **multiple concurrent play worlds** (edit world + N test worlds) in one daemon.
7. **Input** goes through an action-map abstraction, injectable at the action level, with an input log for replay.
8. **One diagnostics bus** with stable codes. Every subsystem (scripting, import, shaders, physics, UI layout) reports through it with structured locations. Source maps for TS.
9. **Asset pipeline**:
   - Text assets load directly with no import step.
   - Binary sources use declarative import sidecars (`id` + settings).
   - Derived data goes to a content-addressed cache.
   - Import, bake and build are jobs with progress.
   - The live reference graph is queryable (who-uses / used-by).
   - A file watcher hot-reloads external edits and validates them.
10. **Persistence is "always saved"**: debounced canonical flush of ops to disk, so there's no hidden dirty state. Canonical formatter + semantic merge driver. Per-entity/cell files for large worlds.
11. **Scripting runtime** with hot reload (module swap, state in components), sandboxable isolates for `sn_run`, a typed SDK generated from reflection, and a test runner with play-world access.
12. **Editor UI (editor track)** must render generically from ops:
    - pending-change overlays in viewport and hierarchy
    - per-field `was → now` in the inspector
    - the Agents panel (Txn timeline, why, before/after, accept/revert)
    - agent presence (selection/camera)
    - a command palette driven by the registry
    - human-action log readable by agents
13. **Game UI system (UI track)**:
    - text-defined
    - web-like layout and style model (flex/grid, CSS-like) to exploit LLM priors
    - computed layout rects queryable
    - renderable in isolation at arbitrary resolution/DPI via `capture.ui`
    - bindings typed against TS
    - one UI system only
14. **Security**: localhost + per-project token, connection approval, capability-scoped sessions enforced in the registry (not the adapter), untrusted-content tagging, and opt-in telemetry (UE defaults analytics on [S6]; don't).
15. **Performance budgets** (gate in CI): headless warm open under 2 s (small project), CLI round-trip under 100 ms, `sn check` under 2 s incremental, hot reload under 1 s, single capture under 500 ms.

---

## 7. Open questions and risks

- **MCP churn.** 2026-07-28 removed sessions and the handshake and deprecated sampling/logging [S16]. Mitigation: own the Engine Protocol and keep MCP a generated adapter. Explicit `session` handles already match the new stateless model.
- **Is TS viable for shipping on consoles and iOS?** JIT bans force an interpreter or AOT, at a performance cost. This must be resolved by the architecture track before committing. Fallback: C#/CoreCLR.
- **Determinism scope.** Physics, threading and GPU readbacks can break replay. Decide early which systems are inside the "deterministic island". Never promise cross-platform lockstep.
- **Text formats at scale.** Parse time and memory for huge worlds. Mitigation: per-cell files plus a binary *cooked* runtime format, with text kept for authoring only.
- **"Always saved" vs user expectations.** Some users want explicit Save. Option: always-saved to a working copy with explicit "commit" (git). Needs UX validation.
- **Review fatigue.** Agents can produce hundreds of ops per turn. Need summarized diffs, grouping by intent, tests and captures as evidence, and trust-based auto-accept policies per scope.
- **Golden-image flakiness** across GPUs and drivers. Use tolerant metrics (FLIP/SSIM) and a pinned software renderer in CI.
- **Graph authoring.** Text graphs are diffable but not automatically *pleasant* for agents. Are expression DSLs for shaders/logic worth the second representation? Evidence favors code over graphs for agents [S24], but artists want graphs. Needs a prototype.
- **Prompt injection via content** (marketplace assets, docs, logs) and **eval escape**. Isolate hardening is security-critical. Treat `sn_run` like a browser sandbox.
- **Model trajectory.** Vision and long-context are improving monthly. Keep primitives raw and composable (query/apply/capture/play) instead of encoding today's prompting tricks. Re-validate with snbench each model generation.
- **Unverified items in this report:**
  - Blender Lab MCP dates and scope (secondary only)
  - the Unity Codex plugin date
  - Godot core team's AI stance
  - Rec Room Maker AI date
  - startup funding figures
  - Unity 7 → CoreCLR (secondary)
  - Astrocade MAU

---

## Sources

**Web**

- [S1] Unity: "Unity's AI tools in beta: How to get started with MCP" (2026-05-11) https://unity.com/en/blog/unity-ai-mcp-how-to-get-started
- [S2] Unity AI Assistant 2.11, MCP overview: https://docs.unity3d.com/Packages/com.unity.ai.assistant@2.11/manual/integration/mcp-overview.html
- [S3] Unity AI Assistant modes: https://docs.unity3d.com/Packages/com.unity.ai.assistant@2.11/manual/about/assistant-modes.html · Unity AI overview: https://unity.com/resources/what-is-unity-ai
- [S4] Unity: "Meet the Unity CLI" (2026-07-20) https://unity.com/blog/meet-the-unity-cli
- [S5] Unity: "Official Unity Plugin for Claude Code" (2026-09-09) https://unity.com/blog/unity-plugin-for-claude-code · Codex (secondary): https://tech-insider.org/unity-plugins-claude-code-codex-2026/
- [S6] Epic: "Unreal MCP in Unreal Editor" (UE 5.8, Experimental) https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-mcp-in-unreal-editor
- [S7] Epic forums: registering Python tools via Unreal MCP (UE 5.8) https://forums.unrealengine.com/t/how-to-register-python-functions-as-ai-callable-tools-through-unreal-mcp-ue-5-8/2741072
- [S8] UE 5.7 release coverage: https://gamefromscratch.com/unreal-engine-5-7-released/ · https://www.cgchannel.com/2025/11/unreal-engine-5-7-five-key-features-for-cg-artists/
- [S9] StraySpark (vendor, biased): https://www.strayspark.studio/blog/ue-57-ai-assistant-vs-mcp-vs-aura-comparison
- [S10] Roblox: "Connect to the Roblox Studio MCP server" https://create.roblox.com/docs/studio/mcp (via search snippet; direct fetch failed DNS)
- [S11] MoguraVR (2026-02-24): https://www.moguravr.com/roblox-studio-mcp-server-agentic-update/
- [S12] TNW (2026-04-16): https://thenextweb.com/news/roblox-ai-assistant-agentic-tools-planning-procedural-models · Roblox newsroom: https://about.roblox.com/newsroom/2026/04/roblox-studio-going-agentic
- [S13] Godot community MCPs: https://github.com/Coding-Solo/godot-mcp · https://godotengine.org/asset-library/asset/5434 · https://www.getdrio.com/mcp/io-github-satelliteoflove-godot-mcp/md
- [S14] Bevy BRP / MCP: https://github.com/natepiano/bevy_brp · https://raw.githubusercontent.com/natepiano/bevy_brp/main/mcp/README.md · https://docs.rs/bevy_remote
- [S15] Blender MCP (secondary): https://www.strayspark.studio/blog/official-blender-mcp-server-comparison-2026 · https://chatforest.com/reviews/blender-mcp-server/ · community: https://github.com/ahujasid/blender-mcp
- [S16] MCP spec 2026-07-28 changelog: https://modelcontextprotocol.io/specification/2026-07-28/changelog
- [S17] MCP Apps: https://blog.modelcontextprotocol.io/tags/apps/
- [S18] Anthropic, "Advanced tool use" (Tool Search Tool): https://www.anthropic.com/engineering/advanced-tool-use
- [S19] Anthropic, "Code execution with MCP" (2025-11-04): https://www.anthropic.com/engineering/code-execution-with-mcp
- [S20] Cloudflare, "Code Mode" (2025-09-26): https://blog.cloudflare.com/code-mode/
- [S21] Agent Client Protocol: https://zed.dev/acp · https://blog.jetbrains.com/ai/2026/01/acp-agent-registry/
- [S22] GameDevBench (arXiv 2602.11103): https://arxiv.org/abs/2602.11103
- [S23] GameLogicBench (arXiv 2609.21562): https://arxiv.org/abs/2609.21562
- [S24] CraftBench-UE (arXiv 2609.23142): https://arxiv.org/abs/2609.23142
- [S25] GameXpert-Bench (arXiv 2608.21833): https://arxiv.org/abs/2608.21833
- [S26] Unity Insight (arXiv 2609.27585): https://arxiv.org/abs/2609.27585
- [S27] GitHub Octoverse 2025: https://github.blog/news-insights/octoverse/octoverse-a-new-developer-joins-github-every-second-as-ai-leads-typescript-to-1/ · https://visualstudiomagazine.com/articles/2025/10/31/typescript-tops-github-octoverse-as-ai-era-reshapes-language-choices.aspx
- [S28] Aura / Ramen: https://www.gamedeveloper.com/press-release/aura-15-0-releases-with-new-features-and-unlimited-usage-for-unreal-engine-and-unity · https://pulse2.com/ramen-acquires-coplay-to-expand-cross-engine-assistant
- [S29] Bezi: https://discussions.unity.com/t/released-bezi-ai-powered-game-development-assistant-for-unity/1708451
- [S30] Moonlake AI: https://www.enterpriseaiworld.com/Articles/News/News/Moonlake-AI-Launches-Open-Beta-for-Its-World-Modeling-Agent-173532.aspx
- [S31] Crunchbase, 2026 gaming funding: https://news.crunchbase.com/venture/2026-global-gaming-startup-funding-up-ai-meshy-decart/ · https://aifundingtracker.com/best-gaming-startups-funding-stats/
- [S32] Google, Project Genie: https://blog.google/innovation-and-ai/models-and-research/google-deepmind/project-genie/
- [S33] Meta Horizon Studio: https://uploadvr.com/meta-horizon-studio-upgrade-ai-assistant-horizon-worlds · https://www.invenglobal.com/articles/26552/meta-reveals-ai-game-creation-tools-horizon-create-and-horizon-studio
- [S34] UEFN AI NPCs: https://respawn.outlookindia.com/gaming/gaming-news/epic-launch-ai-voice-npcs-via-gemini-elevenlabs-in-fortnite-uefn
- [S35] nunu.ai / modl.ai: https://cloud.google.com/blog/products/gaming/co-op-mode-the-ai-partners-driving-the-the-future-of-gaming/ · https://www.ycombinator.com/launches/Jzd-nunu-ai-ai-agents-to-play-and-test-games
- [S36] Rec Room Maker AI: https://blog.recroom.com/posts/maker-ai
- [S37] Set-of-Mark prompting (Yang et al., 2023): https://arxiv.org/abs/2310.11441
- [S38] llms.txt: https://llmstxt.org/

**Reference clones (read-only, `D:\Coding\projects\desktop\snengine\.refs\`)**

- [C1] `godot/main/main.cpp` L517–673, L1464–1943: `--headless`, `--quit-after`, `--write-movie`, `--fixed-fps`, `--doctool`, `--dump-extension-api(-with-docs)` (Godot 4.8-beta per `godot/version.py`)
- [C2] `godot/scene/resources/resource_format_text.cpp` L195–216: `.tscn` node `unique_id` / `parent_id_path`
- [C3] `godot/editor/editor_undo_redo_manager.h` L92–116: `create_action` + hand-written `add_do_method`/`add_undo_method`
- [C4] `godot/scene/main/node.h` L799: `get_configuration_warnings()` returns `PackedStringArray`
- [C5] `godot/doc/classes/` (833 XML class docs); `Node3D.xml` radians note
- [C6] `unity-cs-reference/Editor/Mono/Commands/CommandService.cs`: `[ExcludeFromDocs]` `CommandHandlerAttribute`, `CommandExecuteContext.args : object[]` (Unity 7000.0.0a12 reference, commit bf60283)
- [C7] `unity-cs-reference/Editor/Mono/MenuItem.cs`, `Editor/Mono/Undo/Undo.bindings.cs`, `Editor/Mono/SerializedProperty.bindings.cs`, `Modules/QuickSearch/Editor/QueryEngine/QueryEngine.cs`
- [C8] `o3de/Code/Editor/CryEditPy.cpp` L412 (`ScopeFlags::Automation`, used in 298 .cpp files); `o3de/Gems/EditorPythonBindings/Code/Source/PythonProxyBus.cpp` L407 (`azlmbr`); `.../Source/ActionManager/`
- [C9] `flax/AGENTS.md`; `flax/Source/Engine/Engine/CommandLine.cpp` (`-headless`, `-null`, `-std`, `-build`, `-play`); `flax/Content/` (151 binary `.flax` files)
- [C10] `godot/thirdparty/jolt_physics/Jolt/Core/Core.h` L18, L133: `JPH_CROSS_PLATFORM_DETERMINISTIC` disables FMA
