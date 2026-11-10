# Publication discovery — Mac, 2026-11-10

Project preparation hydrated existing schemas and assembly bytes during discovery, then independently verified the selected modules during their build. It now filters discovery hydration using the same module-selection policy as compilation. Public `--inventory` still validates and hydrates all publications by default; excluded editor modules retain their normal published schema and fingerprint. Ownership-only scans are unchanged.

Selected modules still fully read their current sources, derive configuration/input fingerprints, verify schema and PE hashes, and validate snapshot/PDB identities before returning `cached`. The change removes an earlier duplicate read/parse, not those cache checks. Graph errors, diagnostics, ordering and validation-only write policy remain unchanged.

Read-only probes used two real Rust-published fixtures, four fresh processes ABBA, eight warmups/mode and twenty allocation samples/mode/process. Numbers below are median allocated bytes for the actual discovery method (forty samples per arm/mode), with fixture setup and validation excluded.

| Fixture | Previous discovery | New selected-profile discovery | Public inventory after change |
| --- | ---: | ---: | ---: |
| One source / one module | 24,752 B | 10,016 B | 24,760 B |
| Migrated Outpost / four sources / two modules | 57,672 B | 26,352 B | 57,680 B |

This avoids 14,736 and 31,320 bytes in these discovery phases. The same public/ownership JSON responses match byte for byte at the serialized-response hash level across both revisions. New filtered results are independently checked against public results with only selected modules’ schema/fingerprint/result reset. Fixture, SDK, helper and runtime hashes remain unchanged. Public discovery adds eight closure bytes; required compilation-side cache reads remain. The small fixture savings are not a whole-compile allocation or latency claim.

The first fixture has one valid gameplay publication. Outpost has a valid gameplay publication and an editor module without a published editor image. A separate unit fixture publishes both domains and proves excluded editor hydration, selected cached reads, all-selected editor validation with no private-output writes, corrupt-PE repair, invalid-source rejection without overwriting the last good binary, and rejection of a corrupt excluded editor publication. Existing tests cover PDB and snapshot corruption.

Validation: audited Release build (zero warnings/errors), all 329 .NET tests (233 compiler), eight native project/lifecycle/L1-L2-L3 tests, and exact before/after PE/PDB/schema equality for 96 module pairs from 1,024 scripts. The latter fixture does not publish canonical markers; native/unit cases cover the publication path. One independent read-only review found no production blockers; its optional editor/validation-only case was added. No clocks are accepted and the retained-state p95 question stays open.

[All allocation samples, full publication/SDK/runtime hashes and quality bindings](publication_discovery_mac_2026-11-10.json).

```sh
dotnet build benchmarks/iteration/publication_discovery/Probe.csproj -c Release -m:2 --disable-build-servers
python3 benchmarks/iteration/publication_pairs.py --baseline /absolute/baseline/Ara.CompileServer.dll --candidate /absolute/candidate/Ara.CompileServer.dll --helper target/iteration-perf/publication-discovery/bin/Probe/release/Probe.dll --dotnet-host /absolute/dotnet --projects /absolute/rust-published-project /absolute/another-project --output /absolute/new-results
```

The probe only reads its supplied projects, validates their hashes before/after, and writes evidence to a new output directory. It is optional developer tooling, not an engine build prerequisite.
