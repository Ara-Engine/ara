# L3 analysis scope (Mac, 2026-11-11)

An L3 rebuild runs the analyzers over the whole module in parallel with the full emit. The "parallel" run is not free: with analysis switched off in an experiment build, the median L3 save → visible fell from 238 to 133 ms and the emit itself from 191 to 90 ms (both bind every method body, and they contend). Most L3 edits do not need the whole module analyzed. `AnalysisScope` (generalized from the L2-only version) diffs the changed files' declarations: members matched by kind, name and parameter types, bodies ignored. On a module whose last analysis was clean, with unchanged references and no directives, only the touched files plus the files that spell a changed name (added, removed or changed member or type, a type whose attributes changed, a constructor's type, a namespace no other file declares) can change analyzer results. Whole module as before for: operators, conversions, indexers, finalizers; changed base lists, modifiers, type parameters or primary constructors; assembly attributes, global usings, extern aliases; the names the language binds without spelling them (`GetEnumerator`, `Dispose`, `Add`, `Count`, `ToString`, query operators…); a module with any analyzer diagnostic.

20 fresh processes per arm, mirrored order, 45 edits per process (15 L3 per process, 300 per arm). Median / p95, ms, save → visible (the bench's L3 edit is a signature change, `Shape(int)` ↔ `Shape(long)`):

| | before (L2 scope only) | after |
| --- | ---: | ---: |
| L3 | 237 / 268 | 143 / 165 |
| L3 emit (server, median) | 189 | 95 |
| L2 | 46 / 69 | 46 / 64 |
| L1 | 36 / 147 | 36 / 122 |

Raw rows: `l3_analysis_scope_mac_2026-11-11.json` (`startup_pairs.py` reproduces it). Tests pin the files each kind of change analyzes, the whole-module fallbacks, and a differential case: adding `[Simulation]` to one part of a type must report ARA0200 in another part's file, with exactly the diagnostics a fresh full build reports.
