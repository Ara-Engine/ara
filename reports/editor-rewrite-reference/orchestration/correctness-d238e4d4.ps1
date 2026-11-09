$ErrorActionPreference='Stop'
& 'D:/AraBuild/ux-editor-target/orchestration/monkey-acceptance.ps1' -CheckpointSHA d238e4d4 -FixtureRoot 'D:/AraBuild/ux-editor-target/acceptance/219902aa/behaviours-104729-20261028-011212/captures/behaviour-project-14164' -BuildEvidence 'D:/AraBuild/ux-editor-target/orchestration/windows-build-d238e4d4.log' -EvidenceMode correctness-shared -BeforeMonkeyReport @(
'D:/AraBuild/ux-editor-target/acceptance/219902aa/monkey-104729-20261028-012035/captures/monkey.json',
'D:/AraBuild/ux-editor-target/acceptance/ed243041/monkey-104729-20261028-014243/captures/monkey.json',
'D:/AraBuild/ux-editor-target/acceptance/18bfd589/monkey-104729-20261028-025319/captures/monkey.json'
)
exit $LASTEXITCODE