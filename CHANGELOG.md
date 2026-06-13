# Changelog

## 0.1.0 - 2026-06-13

Initial MVP.

### Added

- Rust CLI with `doctor`, `serve`, `sync stars`, `search stars`, `search github`, `recommend`, `discover from-stars`, and `context`.
- MCP stdio server exposing six agent tools:
  - `search_starred_repositories`
  - `search_github_repositories`
  - `recommend_repositories`
  - `sync_starred_repositories`
  - `discover_repositories_from_starred_profile`
  - `get_repository_context`
- Strict source contract: repository results use only `source = starred` or `source = github`.
- Separate `cache_hit` flag for GitHub search cache state.
- SQLite metadata store for starred repositories and GitHub search cache.
- Tantivy local full-text index for starred repository search.
- GitHub starred repository sync with idempotent upsert.
- GitHub-wide repository search proxy with cache.
- Starred-profile GitHub discovery for high-star recommendations based on local starred languages/topics.
- Local `.env` loading through `dotenvy`, plus `GITSEEK_ENV_FILE` for MCP hosts.
- Hermes MCP registration verified with six discovered tools.

### Fixed

- MCP JSON-RPC notifications without `id` are ignored instead of receiving an invalid `id: null` error response.

### Verified

- `cargo fmt --all`
- `cargo check`
- `cargo test`
- `cargo build --release`
- `hermes mcp test gitseek`
