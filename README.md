# GitSeek

GitSeek is an agent-first GitHub discovery MCP server written in Rust.

It helps coding agents search a user's starred repositories as a local knowledge
base, search GitHub globally, and discover high-star repositories that match the
user's starred-repository profile.

## Contract

GitSeek keeps search source and cache state separate:

- `search_starred_repositories` searches only the local starred repository index.
- `search_github_repositories` searches only GitHub-wide repositories.
- `recommend_repositories` searches starred repositories first, then GitHub-wide repositories as a grouped supplement.
- `discover_repositories_from_starred_profile` builds an interest profile from local stars, then searches GitHub-wide high-star repositories.
- Result `source` is only `starred` or `github`.
- `cache_hit` is an implementation detail and is tracked separately from `source`.

## Features

- MCP stdio server for agent tool-calling.
- CLI for setup, sync, search, and debugging.
- GitHub token auth through `GITHUB_TOKEN` or a local `.env`.
- SQLite metadata store for starred repositories and GitHub search cache.
- Tantivy full-text index for local starred repository search.
- Starred-profile discovery that extracts top languages/topics and recommends high-star GitHub repositories.

## Install

```bash
cargo build --release
```

The binary will be at:

```bash
target/release/gitseek
```

## Configuration

Set a GitHub token before syncing starred repositories or searching GitHub-wide repositories:

```bash
export GITHUB_TOKEN=ghp_your_token
```

For local use, create an ignored `.env`:

```bash
cp .env.example .env
```

For MCP hosts that launch GitSeek outside the project directory, set:

```bash
GITSEEK_ENV_FILE=/absolute/path/to/GitSeek/.env
```

GitSeek also reads `~/.config/gitseek/config.toml` when present.

## CLI

```bash
gitseek doctor
gitseek serve
gitseek sync stars
gitseek search stars "rust mcp server"
gitseek search github "rust mcp server"
gitseek recommend "find rust mcp server examples"
gitseek discover from-stars --min-stars 5000 --limit 10
gitseek context modelcontextprotocol/rust-sdk
```

## MCP Tools

- `search_starred_repositories`
- `search_github_repositories`
- `recommend_repositories`
- `sync_starred_repositories`
- `discover_repositories_from_starred_profile`
- `get_repository_context`

Run the MCP stdio server:

```bash
gitseek serve
```

Example Hermes registration:

```bash
hermes mcp add gitseek \
  --command /absolute/path/to/GitSeek/target/release/gitseek \
  --args serve \
  --env GITSEEK_ENV_FILE=/absolute/path/to/GitSeek/.env
```

## Storage

By default GitSeek uses the platform data directory:

- SQLite metadata: `gitseek/gitseek.sqlite3`
- Tantivy index: `gitseek/tantivy`

Run `gitseek doctor` to see the exact paths on your machine.

## Development

```bash
cargo fmt --all
cargo check
cargo test
cargo build --release
```

## Notes

Tantivy is currently pinned to `=0.25.0`, with `time = 0.3.37`, because newer
`time` versions can trigger an upstream compile conflict in `tantivy-common`
on the current Rust toolchain.
