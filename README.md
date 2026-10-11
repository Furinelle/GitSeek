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
- `discover_starred_for_project` ranks only local stars for a project or a specific need, with explicit match evidence and exclusions.
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
gitseek search stars "rust" --sort starred_at
gitseek search github "rust mcp server"
gitseek recommend "find rust mcp server examples"
gitseek discover from-stars --min-stars 5000 --limit 10
gitseek discover for-project "Telegram folder search" --limit 5
gitseek discover for-project user/my-bot --context project.json --exclude teloxide/teloxide
gitseek context modelcontextprotocol/rust-sdk
```

`gitseek search stars` supports local sort modes:

- `relevance` (default)
- `starred_at`
- `stars`
- `updated`
- `name`

Run `gitseek sync stars` after upgrading from versions before `0.1.1` so the
local SQLite store contains GitHub's real `starred_at` timestamps.

An unlimited, successful sync treats GitHub's complete starred list as a snapshot:
repositories you unstarred are removed locally, including from project discovery.
`--limit` syncs only add or update the returned repositories and never remove unseen
stars. Failed or interrupted list fetches leave the previous snapshot intact; all
SQLite upserts and removals commit in one transaction. An empty complete list clears
the local stars. README excerpts use GitHub's raw text response and keep up to 12,000
Unicode characters; the default sync refreshes older encoded excerpts.

The Tantivy commit records a fingerprint of the SQLite snapshot. If rebuilding the
index fails after SQLite commits, sync returns an error and local search falls back
to SQLite, including after restart. It uses the existing literal substring search
until the next successful sync rebuilds Tantivy. Older indexes without a fingerprint
also use this fallback; run `gitseek sync stars` to restore full-text relevance.

### Find a use for tools you already starred

`discover for-project` runs entirely locally after stars have been synced. Pass
a short need, or supply project metadata in a JSON file:

```json
{
  "full_name": "user/my-bot",
  "description": "Private Telegram folder search bot with caching",
  "language": "Rust",
  "topics": ["telegram", "search"],
  "used_repositories": ["teloxide/teloxide"]
}
```

GitSeek reads only the explicitly supplied JSON file, never a project directory
or its manifests. `owner/repo` targets without `--context` resolve only when the
target is itself in the local starred store; otherwise describe the need or
provide context. The target must match `context.full_name` when using `owner/repo`.
The target itself, `used_repositories`, and `--exclude` names are excluded without
regard to case. Dependency names must be supplied explicitly as `owner/repo`;
package names are not automatically mapped to GitHub repositories.

Every match has `source: "starred"`, a `relevance_score`, and `why_matched`
listing the exact terms found in each field. Ranking weights topics (4), name
(3), description (2), and cached README (1), with rarer terms weighted more.
`purpose_terms` come from the short need, the project's first description sentence,
and explicit topics; these terms get three times the weight of later context.
The score is then multiplied by the number of distinct matched purpose terms
(at least one), so a single rare ambiguous word does not dominate broader fit.
Star counts never affect ranking. The context's implementation language is
removed from query terms and only adds a supporting tie-break point to existing
matches. No functional term match means no candidate.

This is lexical discovery, not an integration or quality assessment: no synonym
translation, stemming, or Chinese word segmentation is performed. Use concise
keywords present in your stars' metadata; missing metadata can hide useful tools.
Project-description sentences with explicit English negative requirements such
as "do not require" or "does not use" are omitted so those dependencies do not
become search needs. Other negation is not interpreted.
Read each candidate's documentation and license before adopting it. No GitHub or
LLM request is made, and the tool does not modify repositories or stored stars.

To keep an evaluation separate from your regular database, all CLI commands
accept `--data-dir /path/to/isolated-data` or `GITSEEK_DATA_DIR`.

## MCP Tools

- `search_starred_repositories`
- `search_github_repositories`
- `recommend_repositories`
- `sync_starred_repositories`
- `discover_repositories_from_starred_profile`
- `discover_starred_for_project`
- `get_repository_context`

The project discovery MCP tool accepts `target`, optional inline `context` with
the same fields as the JSON above, `exclude_full_names`, and `limit` (1–50).

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
