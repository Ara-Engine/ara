# Documentation parsing — Mac, 2026-11-10

Plain field/type summaries incurred a fresh XML parser and element tree on every generator extraction. `Docs.Tag` now recognizes one restricted shape directly: `<member>` or `<member name="…">`, exactly one attribute-free `<summary>` containing plain ASCII text, then the closing member. XML whitespace is permitted between elements and normalized by the existing `Collapse` function. Quotes and XML metacharacters are checked in the optional name attribute. Text containing entities, markup, Unicode, invalid controls or a raw `]]>` cannot take the shortcut. Attributes on summaries, namespaces, siblings, comments, CDATA, declarations and all unrecognized inputs use the original full XML parser. Parameter documentation retains its original path.

This adds no cache or gameplay state and changes no generated registration, diagnostics or schema semantics. A source-only independent adversarial review found no malformed XML accepted by the restricted grammar. Full XML remains the correctness fallback, including rejection of malformed tails after an otherwise valid summary.

## Exact counters and output quality

Main `5794174e8` versus only the documentation change; this is independent of PR #16. Same complete dependency closure, actual compiler CLI, 8 modules × 128 component/query scripts, one edited file/module/request, four fresh processes ABBA, five warmups and ten measured requests/process. Observed runtime: CoreCLR 10.0.7 on macOS arm64. Generator DLL/PDB and compiler DLL/PDB rebuilt against that generator differ; common dependency bytes match. All SDK/hook/harness hashes are recorded and stable.

| Observation | Previous | Plain-summary recognition |
| --- | ---: | ---: |
| Median allocated B/request, 20 measured requests/arm | 699,976,216 | 639,283,120 |
| Min–max allocated B/request | 697,448,416–703,490,440 | 637,157,808–642,942,208 |
| Final forced-GC live heap, process 1 | 170,017,344 | 167,468,368 |
| Final forced-GC live heap, process 2 | 169,202,328 | 167,897,544 |
| Median final live heap, two processes/arm | 169,609,836 | 167,682,956 |

This fixture allocates **60,693,096 fewer bytes/request (8.67%)**. Final live-heap median is 1,926,880 bytes lower, but two instrumented observations per arm do not establish a general retention/p95 improvement. Final heap uses collect/finalize/collect while compiler state and last reply remain rooted; the final request includes that forced GC. Hook logging is outside allocation boundaries but can shift GC phase. Shared-host activity and hook overhead invalidate **all clocks**. No compile-time, p95, peak-RSS, time-to-Play or Unity/ECS superiority claim follows from these counters.

All 60 compiler requests pass, with **360 exact corresponding PE/PDB/canonical-schema module comparisons**. The 13,720-case XML differential corpus produces identical summary and named-parameter outputs before/after (27,440 text comparisons). It includes all byte-range characters at wrapper/text boundaries, rich XML, Unicode, entities, namespaces, attributes, duplicate summaries, malformed tails, token deletion/insertion and deterministic randomized damage. Each arm has an independently hashed raw output; the corpus, helper and complete SDK manifests remain unchanged.

All **343 .NET tests** pass: compiler 233, generator 43, analyzer 30, runtime 28, worker 9. The audited Release build has zero warnings/errors. Fourteen new documentation cases exercise syntax-node and symbol entry points under both `DocumentationMode.None` and `Parse`. Ten native scripting tests pass across batch queries, publication, project/compile lifecycle and reload levels. Formatting passes all 116 packages. No Rust production files change.

The optional differential helper initially had an extra closing parenthesis (fixed; helper build 02 passed). The first corpus invocation then failed before documentation extraction because System.Text.Json rejects an unpaired-surrogate JSON string. That receipt remains retained; the successful corpus uses transported U+2028 instead. These results do not claim lone-surrogate coverage. Neither failure was a compiler quality failure.

## Reproduction and limits

The allocation runner and hook are the existing `retained_gc.py` and `project_gc/Hook.csproj`. Run four ABBA processes with `--modules 8 --files 128 --warmups 5 --samples 10`, supplying complete frozen `--baseline`/`--candidate` server paths, `--hook`, `--dotnet-host` and a fresh `--output` directory. `doc_pairs.py` constructs the deterministic XML corpus and runs the reflection helper in `doc_quality/` against each frozen generator. This optional probe calls the actual private extraction entry without adding an engine API:

```sh
dotnet build benchmarks/iteration/doc_quality/Probe.csproj -c Release --disable-build-servers
python3 benchmarks/iteration/doc_pairs.py \
  --baseline /absolute/baseline-sdk --candidate /absolute/candidate-sdk \
  --helper "$PWD/target/iteration-perf/doc-quality/bin/Probe/release/Probe.dll" \
  --dotnet-host /absolute/dotnet --output /absolute/fresh-output
```

`doc_parsing_allocations_mac_2026-11-10.json` retains all compiler request counters and exact output hashes, per-process summaries, loaded runtime images, SDK/source/helper/test proof hashes and differential receipts. Raw local traces and failed probe outputs remain under `target/script-overnight-20261110/doc-parsing/`. `plot_compiler_allocations.py` regenerates the graph from this packet. Diagnostic helpers/Python/matplotlib are optional and are not engine build prerequisites.

The warmed compiler profile that selected this work is documented in PR #16's `QUERY_TAG_DISCOVERY.md`; its allocation ticks are sampled weights, not exact per-site bytes or CPU shares. This comparison starts from the same main as #16, so the two measured savings overlap and **must not be added**. Combined-main measurements are required after integration. Dependency reverse pairs and the historical retained-state p95 question remain open; no rejected historical clocks have been reclassified.

The next correctness prerequisite for broader generator reuse is proving that documentation moved between declarations invalidates schema extraction even when concatenated comment text is unchanged. A hot-edit investigation has started there before any further generator skipping.
