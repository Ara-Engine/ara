# L2 analysis scope (Mac, 2026-11-11)

Profile of an additive (L2) edit in `reload_bench` (5.2k lines, 88 files): the EnC emit takes 3 ms, but the analyzers re-ran over the whole module, 90–120 ms of a 134 ms save → visible. An L2 edit adds members. New members change how other code binds only where that code spells the name, so when the module's last analysis was clean (the proof L1 body edits already use) the touched files plus every file that spells an inserted name are enough (`AdditiveAnalysisScope`). Types, operators and conversions (not bound by a spelled name), and the names the language looks up without spelling them (`GetEnumerator`, `Dispose`, `Add`, `Count`, `ToString`, query operators…), still take the whole module, as do edits on a module with any analyzer diagnostic, changed references, directives or partial-method bodies. Analyzer results, error text and the retained-on-error state are unchanged.

20 fresh processes per arm, mirrored order (base, l2, l2, base ×10), 45 edits per process (15 L2 per process, 300 per arm), Mac shared with other agents. Median / p95, ms, save → visible:

| | before | after |
| --- | ---: | ---: |
| L2 | 134 / 161 | 47 / 69 |
| L2 analyzers (server) | 91.7 median | 1.9 median |
| L1 | 34 / 139 | 36 / 142 |
| L3 | 236 / 267 | 236 / 260 |

L1 and L3 are untouched by the change and match within noise. Raw rows: `l2_analysis_scope_mac_2026-11-11.json`; `startup_pairs.py` reproduces it. Nine new session tests pin which files each kind of insert analyzes, the whole-module fallbacks, a not-clean baseline, and error retention; the .NET and native reload suites pass.
