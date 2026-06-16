use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{RepositoryRecord, RepositorySource};

pub struct RepositoryStore {
    conn: Connection,
}

impl RepositoryStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create data dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open SQLite database {}", path.display()))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS repositories (
                github_id INTEGER PRIMARY KEY,
                owner TEXT NOT NULL,
                name TEXT NOT NULL,
                full_name TEXT NOT NULL UNIQUE,
                url TEXT NOT NULL,
                description TEXT,
                homepage TEXT,
                topics TEXT NOT NULL DEFAULT '[]',
                language TEXT,
                license TEXT,
                stars INTEGER NOT NULL DEFAULT 0,
                forks INTEGER NOT NULL DEFAULT 0,
                watchers INTEGER NOT NULL DEFAULT 0,
                created_at TEXT,
                updated_at TEXT,
                pushed_at TEXT,
                starred_at TEXT,
                last_synced_at TEXT,
                readme_fetched_at TEXT,
                etag TEXT,
                source TEXT NOT NULL,
                readme TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_repositories_source ON repositories(source);
            CREATE INDEX IF NOT EXISTS idx_repositories_owner ON repositories(owner);
            CREATE INDEX IF NOT EXISTS idx_repositories_language ON repositories(language);

            CREATE TABLE IF NOT EXISTS github_search_cache (
                cache_key TEXT PRIMARY KEY,
                response_json TEXT NOT NULL,
                fetched_at TEXT NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    pub fn upsert_repository(&self, repo: &RepositoryRecord) -> Result<bool> {
        let changed = self.conn.execute(
            r#"
            INSERT INTO repositories (
                github_id, owner, name, full_name, url, description, homepage, topics,
                language, license, stars, forks, watchers, created_at, updated_at,
                pushed_at, starred_at, last_synced_at, readme_fetched_at, etag, source, readme
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
            ON CONFLICT(github_id) DO UPDATE SET
                owner = excluded.owner,
                name = excluded.name,
                full_name = excluded.full_name,
                url = excluded.url,
                description = excluded.description,
                homepage = excluded.homepage,
                topics = excluded.topics,
                language = excluded.language,
                license = excluded.license,
                stars = excluded.stars,
                forks = excluded.forks,
                watchers = excluded.watchers,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                pushed_at = excluded.pushed_at,
                starred_at = excluded.starred_at,
                last_synced_at = excluded.last_synced_at,
                readme_fetched_at = excluded.readme_fetched_at,
                etag = excluded.etag,
                source = excluded.source,
                readme = COALESCE(excluded.readme, repositories.readme)
            "#,
            params![
                repo.github_id,
                repo.owner,
                repo.name,
                repo.full_name,
                repo.url,
                repo.description,
                repo.homepage,
                serde_json::to_string(&repo.topics)?,
                repo.language,
                repo.license,
                repo.stars as i64,
                repo.forks as i64,
                repo.watchers as i64,
                fmt_time(repo.created_at),
                fmt_time(repo.updated_at),
                fmt_time(repo.pushed_at),
                fmt_time(repo.starred_at),
                fmt_time(repo.last_synced_at),
                fmt_time(repo.readme_fetched_at),
                repo.etag,
                source_to_str(repo.source),
                repo.readme,
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn all_starred(&self) -> Result<Vec<RepositoryRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM repositories WHERE source = 'starred' ORDER BY starred_at DESC, stars DESC",
        )?;
        let rows = stmt.query_map([], row_to_repo)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect starred repository rows")
    }

    pub fn find_by_full_names(&self, full_names: &[String]) -> Result<Vec<RepositoryRecord>> {
        let mut repos = Vec::new();
        for full_name in full_names {
            if let Some(repo) = self
                .conn
                .query_row(
                    "SELECT * FROM repositories WHERE full_name = ?1",
                    [full_name],
                    row_to_repo,
                )
                .optional()?
            {
                repos.push(repo);
            }
        }
        Ok(repos)
    }

    pub fn text_search_starred(
        &self,
        query: &str,
        limit: usize,
        sort: Option<&str>,
    ) -> Result<Vec<RepositoryRecord>> {
        let like = format!("%{}%", query.to_ascii_lowercase());
        let order_by = match normalize_starred_sort(sort) {
            StarredSort::StarredAt => "starred_at DESC, stars DESC",
            StarredSort::Stars => "stars DESC, updated_at DESC",
            StarredSort::Updated => "updated_at DESC, stars DESC",
            StarredSort::Name => "lower(full_name) ASC",
            StarredSort::Relevance => "stars DESC, updated_at DESC",
        };
        let sql = format!(
            r#"
            SELECT * FROM repositories
            WHERE source = 'starred'
              AND (
                lower(full_name) LIKE ?1
                OR lower(description) LIKE ?1
                OR lower(topics) LIKE ?1
                OR lower(language) LIKE ?1
                OR lower(readme) LIKE ?1
              )
            ORDER BY {order_by}
            LIMIT ?2
            "#
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![like, limit as i64], row_to_repo)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect starred search rows")
    }

    pub fn cached_search(&self, cache_key: &str) -> Result<Option<(String, DateTime<Utc>)>> {
        self.conn
            .query_row(
                "SELECT response_json, fetched_at FROM github_search_cache WHERE cache_key = ?1",
                [cache_key],
                |row| {
                    let json: String = row.get(0)?;
                    let fetched_at: String = row.get(1)?;
                    Ok((json, parse_time_required(&fetched_at)))
                },
            )
            .optional()
            .context("failed to read GitHub search cache")
    }

    pub fn put_cached_search(&self, cache_key: &str, response_json: &str) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO github_search_cache (cache_key, response_json, fetched_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(cache_key) DO UPDATE SET
                response_json = excluded.response_json,
                fetched_at = excluded.fetched_at
            "#,
            params![cache_key, response_json, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StarredSort {
    Relevance,
    StarredAt,
    Stars,
    Updated,
    Name,
}

pub(crate) fn normalize_starred_sort(sort: Option<&str>) -> StarredSort {
    match sort.unwrap_or("relevance").to_ascii_lowercase().as_str() {
        "starred" | "starred_at" | "starred-at" | "created" => StarredSort::StarredAt,
        "stars" | "stargazers" => StarredSort::Stars,
        "updated" | "updated_at" | "updated-at" | "pushed" | "pushed_at" | "pushed-at" => {
            StarredSort::Updated
        }
        "name" | "full_name" | "full-name" => StarredSort::Name,
        _ => StarredSort::Relevance,
    }
}

fn row_to_repo(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepositoryRecord> {
    let topics_json: String = row.get("topics")?;
    let topics = serde_json::from_str(&topics_json).unwrap_or_default();
    let source: String = row.get("source")?;
    Ok(RepositoryRecord {
        github_id: row.get("github_id")?,
        owner: row.get("owner")?,
        name: row.get("name")?,
        full_name: row.get("full_name")?,
        url: row.get("url")?,
        description: row.get("description")?,
        homepage: row.get("homepage")?,
        topics,
        language: row.get("language")?,
        license: row.get("license")?,
        stars: row.get::<_, i64>("stars")? as u64,
        forks: row.get::<_, i64>("forks")? as u64,
        watchers: row.get::<_, i64>("watchers")? as u64,
        created_at: row
            .get::<_, Option<String>>("created_at")?
            .and_then(parse_time),
        updated_at: row
            .get::<_, Option<String>>("updated_at")?
            .and_then(parse_time),
        pushed_at: row
            .get::<_, Option<String>>("pushed_at")?
            .and_then(parse_time),
        starred_at: row
            .get::<_, Option<String>>("starred_at")?
            .and_then(parse_time),
        last_synced_at: row
            .get::<_, Option<String>>("last_synced_at")?
            .and_then(parse_time),
        readme_fetched_at: row
            .get::<_, Option<String>>("readme_fetched_at")?
            .and_then(parse_time),
        etag: row.get("etag")?,
        source: if source == "github" {
            RepositorySource::Github
        } else {
            RepositorySource::Starred
        },
        readme: row.get("readme")?,
    })
}

fn source_to_str(source: RepositorySource) -> &'static str {
    match source {
        RepositorySource::Starred => "starred",
        RepositorySource::Github => "github",
    }
}

fn fmt_time(time: Option<DateTime<Utc>>) -> Option<String> {
    time.map(|time| time.to_rfc3339())
}

fn parse_time(value: String) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn parse_time_required(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let store = RepositoryStore::open(&temp.path().join("gitseek.sqlite3")).unwrap();
        let repo = fixture_repo();

        store.upsert_repository(&repo).unwrap();
        store.upsert_repository(&repo).unwrap();

        let starred = store.all_starred().unwrap();
        assert_eq!(starred.len(), 1);
        assert_eq!(starred[0].full_name, "modelcontextprotocol/rust-sdk");
    }

    fn fixture_repo() -> RepositoryRecord {
        RepositoryRecord {
            github_id: 1,
            owner: "modelcontextprotocol".to_string(),
            name: "rust-sdk".to_string(),
            full_name: "modelcontextprotocol/rust-sdk".to_string(),
            url: "https://github.com/modelcontextprotocol/rust-sdk".to_string(),
            description: Some("Rust SDK for MCP".to_string()),
            homepage: None,
            topics: vec!["mcp".to_string(), "rust".to_string()],
            language: Some("Rust".to_string()),
            license: None,
            stars: 42,
            forks: 1,
            watchers: 42,
            created_at: None,
            updated_at: None,
            pushed_at: None,
            starred_at: None,
            last_synced_at: Some(Utc::now()),
            readme_fetched_at: None,
            etag: None,
            source: RepositorySource::Starred,
            readme: Some("Build MCP servers in Rust".to_string()),
        }
    }
}
