# ara-update

Shared signed release protocol. No UI, engine authority or automatic startup network
access. `Feed` checks GitHub with per-page ETags and a signed offline cache;
`VerifiedRelease` is the only input accepted by downloads. `download` streams into
a resumable staging file; `extract` rejects unsafe ZIP paths and expansion bombs.
Asset transfers run a private Tokio runtime on the I/O worker so Cancel interrupts
stalled headers/body reads; metadata uses reqwest's bounded blocking client.
`updater` swaps only a standalone Hub directory, with rollback and health checks.

Default repository: Snowy7/ara. `ARA_RELEASE_REPO` overrides update.json's repo.
`GITHUB_TOKEN` is sent only to api.github.com, never to redirected asset hosts.
Trust roots come from embedded public-keys.txt and build-time ARA_RELEASE_PUBLIC_KEYS
(comma-separated hexadecimal ed25519 public keys). Never embed a private key.
An empty trust store fails closed. Key rotation: ship both keys before changing signer.

Every test uses loopback HTTP via the opt-in test-support feature or cfg(test).
Set ARA_CONFIG_DIR to an existing scratch dir under target/ for tests, captures
and manual checks. tests/config_isolation.rs is also included by Hub/CLI/MCP/
release tests and fails if isolation is missing. Dummy updater children are awaited.
Production accepts HTTPS GitHub API/assets only; no runtime insecure switch.
Cache is scoped to repo/API and verified again on every read. Rate limits/offline
fall back only for transport errors, never for invalid signatures or manifests.

Gates: cargo test -p ara-update; cargo clippy -p ara-update --all-targets -- -D warnings.
Tests and captures live under target. Diagnostics: ARA1950–ARA1959 in
ara-diag/codes/subsystems.md.
