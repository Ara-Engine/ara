# ara-agents

Connects AI agent clients (Claude Code, Codex, Cursor, OpenCode) to Ara's global MCP server (`<ara> mcp`, no project root). The server sees every project registered in the Hub.

Shared by:
- the Hub's Agents page (`ara-hub`);
- the editor's Connect an agent dialog (`ara-editor`);
- `ara hub install-mcp --client …` (`ara`).

All three detect, register and print config entries the same way.

## Public API

- `Client`:
  - `ALL`, in display order;
  - `name()` for display, e.g. "Claude Code";
  - `id()`, the `--client` value and asset name;
  - `logo()`, the SVG mark;
  - `manual_hint()`, the caption for `manual`.
- `Setup`: `Missing`, `Available`, `Working`, `Connected` or `Failed(reason)`.
- `detect(client)`: whether the client is installed and has Ara registered. It runs `claude`/`codex`/`opencode --version` and reads the config files, so it blocks; call it off the UI thread.
- `setup(client, ara)`: registers `<ara> mcp` at user scope. Blocks.
  - Claude Code: `claude mcp add --scope user ara -- <ara> mcp`.
  - Codex: appends `[mcp_servers.ara]` to `~/.codex/config.toml`, where `CODEX_HOME` overrides the folder.
  - Cursor: merges into `~/.cursor/mcp.json`.
  - OpenCode: merges into `$XDG_CONFIG_HOME|~/.config/opencode/opencode.json(c)`.
- `snippet(ara, client)`: the config entry. It is JSON for Claude Code, Cursor and OpenCode, and TOML for Codex.
- `manual(ara, client)`: the copyable manual step. For Claude Code it is the one-line `claude mcp add`; for the others it is the config entry.

## Testing

`cargo test -p ara-agents` merges JSON config into a temp folder; it never touches the real user's agent configs.

## Gotchas

- JSON configs keep every other setting and their key order. A config with comments (JSONC) is refused, never rewritten, and the manual steps cover it.
- On Windows, npm installs `.cmd` shims, so client CLIs run through `cmd /C`. Every child process uses `CREATE_NO_WINDOW`, so GUIs never flash a console.
- Paths drop the `\\?\` verbatim prefix (`dunce`), so configs show plain paths.
- Logos are Simple Icons (CC0, `assets/LICENSE`), listed in `THIRD_PARTY_NOTICES.md`.
