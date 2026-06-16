use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    StarredOnly,
    GithubOnly,
    StarredFirstThenGithub,
    StarredProfileGithubDiscovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositorySource {
    Starred,
    Github,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routing {
    pub mode: SearchMode,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub language: Option<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    pub owner: Option<String>,
    pub limit: Option<usize>,
    pub sort: Option<String>,
    pub min_stars: Option<u64>,
    pub updated_after: Option<String>,
    pub min_github_results: Option<usize>,
}

impl SearchRequest {
    #[must_use]
    pub fn limit_or(&self, default_limit: usize) -> usize {
        self.limit.unwrap_or(default_limit).clamp(1, 50)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileDiscoveryRequest {
    pub limit: Option<usize>,
    pub min_stars: Option<u64>,
    pub top_languages: Option<usize>,
    pub top_topics: Option<usize>,
    #[serde(default)]
    pub include_languages: Vec<String>,
    #[serde(default)]
    pub include_topics: Vec<String>,
    #[serde(default)]
    pub exclude_full_names: Vec<String>,
}

impl ProfileDiscoveryRequest {
    #[must_use]
    pub fn limit_or(&self, default_limit: usize) -> usize {
        self.limit.unwrap_or(default_limit).clamp(1, 50)
    }

    #[must_use]
    pub fn min_stars_or_default(&self) -> u64 {
        self.min_stars.unwrap_or(1_000)
    }

    #[must_use]
    pub fn top_languages_or_default(&self) -> usize {
        self.top_languages.unwrap_or(3).clamp(1, 10)
    }

    #[must_use]
    pub fn top_topics_or_default(&self) -> usize {
        self.top_topics.unwrap_or(8).clamp(1, 20)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSignal {
    pub value: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarredRepositoryProfile {
    pub starred_count: usize,
    pub top_languages: Vec<ProfileSignal>,
    pub top_topics: Vec<ProfileSignal>,
    pub seed_repositories: Vec<String>,
    pub github_query: String,
    pub min_stars: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryResult {
    pub full_name: String,
    pub url: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub stars: u64,
    pub starred_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub topics: Vec<String>,
    pub source: RepositorySource,
    pub cache_hit: bool,
    #[serde(default)]
    pub why_matched: Vec<String>,
    pub recommended_use: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchResponse {
    Single {
        routing: Routing,
        matches: Vec<RepositoryResult>,
    },
    Grouped {
        routing: Routing,
        starred_matches: Vec<RepositoryResult>,
        github_matches: Vec<RepositoryResult>,
        summary: String,
    },
    ProfileDiscovery {
        routing: Routing,
        profile: StarredRepositoryProfile,
        github_matches: Vec<RepositoryResult>,
        summary: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    pub synced_count: usize,
    pub updated_count: usize,
    pub removed_count: usize,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryRecord {
    pub github_id: i64,
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub url: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub topics: Vec<String>,
    pub language: Option<String>,
    pub license: Option<String>,
    pub stars: u64,
    pub forks: u64,
    pub watchers: u64,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub pushed_at: Option<DateTime<Utc>>,
    pub starred_at: Option<DateTime<Utc>>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub readme_fetched_at: Option<DateTime<Utc>>,
    pub etag: Option<String>,
    pub source: RepositorySource,
    pub readme: Option<String>,
}

impl RepositoryRecord {
    #[must_use]
    pub fn to_result(&self, source: RepositorySource, cache_hit: bool) -> RepositoryResult {
        RepositoryResult {
            full_name: self.full_name.clone(),
            url: self.url.clone(),
            description: self.description.clone(),
            language: self.language.clone(),
            stars: self.stars,
            starred_at: self.starred_at,
            topics: self.topics.clone(),
            source,
            cache_hit,
            why_matched: Vec::new(),
            recommended_use: None,
        }
    }
}
