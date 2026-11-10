# Primary-constructor capture across partial hot edits

Main `3efd0e608` can accept an L1 delta after a method in one partial declaration starts capturing a primary-constructor parameter declared in another file. This synthesizes an instance field and can make a previously unmanaged component managed. Fresh generation reports ARA0703 for an undocumented float capture, or ARA0704 for a string capture; the old live path skipped those checks. Two direct session tests reproduced `Delta` instead of `Errors` on main.

Adding a capturing method to an existing primary-constructor class also changes its constructor's initialization code. Two more tests reproduced L2 deltas where the existing constructor requires a full image. A constructor-local syntax check cannot see the parameter list in another partial file.

The semantic guard now resolves owning types across all partial declarations and enclosing types. Existing bodies use committed semantics; inserted members use the updated compilation. Newly inserted types/delegates remain eligible for their existing L2 route because their full new definitions already include constructors. Field insertions resolve each variable's symbol. Unknown ownership falls back to a full constructor rebuild. A temporary checked-owner set avoids repeatedly scanning a large partial type's declarations; no retained cache or gameplay state is added.

Invalid component captures report the normal generator errors and keep the committed state; valid constructor changes produce L3 images. Unrelated types still take L1, and ordinary const/readonly-static field additions still take L2. Seven focused session cases cover these paths and repair. All 43 constructor/classifier cases, **361 .NET tests**, ten native scripting tests and formatting of 116 packages pass. Audited Release build: zero warnings/errors. The native reload-level test applies code and verifies gameplay; diagnostic allocation probes stop at compiler candidate production.

## Allocation check and limits

Four fresh processes ABBA, five warmups and twenty measured ordinary L1 edits/process, fixed SDK dependencies, observed runtime 10.0.7 on macOS arm64. Actual `ModuleSession.Compile` plus commit, including full source reads and policy checks:

| Accepted fixture | Previous median B/edit | Safety guard median B/edit | Change |
| --- | ---: | ---: | ---: |
| One script | 620,992 | 621,296 | +304 B (+0.049%) |

All initial PE/PDB bytes and all 25 corresponding delta-IL hashes match across the four processes. Each candidate is nonempty L1, has stable generation and adds zero metadata fields. EnC metadata/PDB raw hashes remain retained; their producer identities are not treated as byte-parity evidence. SDK/helper hashes remain unchanged, including a separate after-failure audit. This is an allocation observation only, with no latency, p95, RSS or runtime-ApplyUpdate claim.

The larger 129-script cell failed initial PE/PDB identity equality despite matching all delta IL hashes. Its numbers are **rejected**, and the following bulk-partial case never launched. Initial compilation currently enumerates `ImmutableDictionary.Values` without an explicit source order; this is a separate reproducibility investigation, not a reason to normalize away differing images.

Optional reproduction: build `hot_edit_gc/Probe.csproj` with `-p:AraSdk=/absolute/frozen-sdk`, then run `hot_edit_pairs.py --baseline /absolute/baseline-sdk --candidate /absolute/candidate-sdk --helper /absolute/Ara.CompileServer.Tests.dll --dotnet-host /absolute/dotnet --output /absolute/new-output`. The executable uses the existing compiler-test friendship and is never shipped. Its captures are private to `target/`; no test writes tracked data or the real user profile.

Raw rows, delta hashes, frozen manifests and logs stay local, under `target/script-overnight-20261110/primary-safety/`; they are not committed. The separate project-generator optimization remains unshipped until it incorporates this constructor ownership check and passes fresh-output equivalence.
