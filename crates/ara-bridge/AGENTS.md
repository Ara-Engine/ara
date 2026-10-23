# ara-bridge

Authenticated localhost WebSocket observability for development/profile players.
`Server::start` owns a bounded worker thread; `Publisher::try_publish` never blocks
a tick. `RuntimeHooks` is the renderer/simulation boundary. Implement hooks by
queuing typed input actions for a tick boundary, never by editing documents.
Release runtimes must not start this server. Tokens come from launch configuration.
`DeviceClient` speaks the same protocol over desktop TCP or `Adb::forward`.

Run `cargo test -p ara-bridge`, `cargo clippy -p ara-bridge --all-targets`.
No globals, unbounded logs, background services, or new runtime/gameplay state.
Screenshot and semantic scene APIs fail with stable diagnostics until hooks exist.
