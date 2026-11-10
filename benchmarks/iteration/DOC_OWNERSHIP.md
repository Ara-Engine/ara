# Preserve documentation ownership during hot edits

Main `5794174e8` compared documentation by concatenating all doc-comment text in each changed file. Moving a field's unchanged comment onto an unreflected helper preserves that concatenation. The generator was skipped, so the now-undocumented reflected field bypassed ARA0703. A documentation-only save returned `None`; the same move plus a gameplay method edit returned an L1 `Delta`.

Two actual session tests reproduced those failures on main. The change compares each comment's text and syntax attachment: child-slot path from root to token, leading/trailing trivia side and index. These paths do not depend on source offsets, so changing an earlier method body's length does not move a later field's documentation. A single explicit-stack traversal skips subtrees with no structured trivia; it adds no retained cache, recursion, gameplay state or generator API. `ModuleSession` calculates the guard once per change set instead of twice for ordinary L1 method edits.

Moved comments now rerun generation. Invalid saves report ARA0703 and retain the exact committed state; repaired bodies still produce L1 deltas. Valid changes to reflected field docs produce an L3 `Generated` image and advance generation once. Ordinary body edits retain their existing classification. Moving ordinary trivia before a doc can conservatively rerun generation; if generated output stays identical, that alone does not force L3.

## Verification

- Original red run: two failing missing-documentation cases (`None`/`Delta` instead of `Errors`); two related valid schema-change cases already passed.
- Final focused run: all five pass, including body-edit variants, last-good retention, repair, changed-schema generation and unchanged attachment after body-length changes.
- All **334 .NET tests** pass: compiler 238, generator 29, analyzer 30, runtime 28, worker 9. That initial proof is before integrating PR #16; PR #17 is separate. After a real merge of main `2f72e2de5` (including editor #6 and scripting #16), all **340 .NET tests**, ten native scripting tests and formatting pass again. The final Release solution build has zero warnings/errors. Own source blobs are unchanged by the merge; initial guard counters remain explicitly bound to the earlier main rather than being relabeled as new measurements.
- Ten native scripting tests pass across batch queries, publication, project/compile lifecycle and reload levels. Formatting passes all 116 packages. Audited Release solution build has zero warnings/errors.
- Independent source review found no must-fix in the final traversal. No code is copied from reference engines. ECS state and deterministic simulation behavior are unchanged.

## Guard-phase allocation check

Four fresh processes ABBA, eight warmups and twenty measured calls/case/process, same parsed inputs, actual private `DocsChanged` entry, observed runtime 10.0.7, macOS arm64. Forty samples per arm/case. Parsing, fixture construction, loading and serialization are outside allocation boundaries; reflection return boxing is included equally. Each check correctly returns false for an ordinary method-body replacement. Complete SDK/helper manifests remain stable.

| Documented fields in changed file | Previous B/guard call | Correct attachment B/guard call |
| --- | ---: | ---: |
| 0 | 728 | 328 |
| 1 | 1,016 | 1,336 |
| 128 | 54,968 | 50,488 |
| 1,024 | 438,552 | 408,888 |

One documented field adds 320 bytes per call. Successful ordinary L1 method edits previously evaluated this guard twice and now do so once. These are guard-phase counters only; there is no whole-compiler allocation, latency, p95, time-to-Play or ECS-performance conclusion.

An intermediate dictionary-based attachment prototype passed correctness but allocated 2,039,496 bytes/call for 1,024 fields. It was replaced before shipping; its raw per-case summary is retained. Two intermediate builds caught unavailable Roslyn API names (`ChildSyntaxList.IndexOf`, `ContainsStructuredTrivia`); corrected code uses a traversal and documented `HasStructuredTrivia`. Failed logs remain bound, alongside successful final evidence.

`doc_ownership_quality_mac_2026-11-10.json` retains every allocation sample, SDK/helper hashes, source and test-log hashes, red/green receipts and rejected-prototype counters. Optional probe source is `doc_guard/`:

```sh
dotnet build benchmarks/iteration/doc_guard/Probe.csproj -c Release --disable-build-servers
dotnet target/iteration-perf/doc-guard/bin/Probe/release/Probe.dll /absolute/frozen-sdk
```

Run that probe in baseline → candidate → candidate → baseline order; `plot_doc_guard.py` regenerates the graph. Diagnostic helpers are optional, not engine build prerequisites. Local raw evidence remains in `target/script-overnight-20261110/doc-ownership/`.

This correctness prerequisite was found while investigating broader generator reuse. It does not enable additional skipping. Dependency reverse pairs and historical retained-state p95 remain open: the two latest timing attempts (retained window 04, dependency window 05) were rejected at preflight after competing builds appeared; neither launched a compiler or produced request timings. Existing rejected evidence remains unchanged.
