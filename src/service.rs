use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::{
    config::Config,
    github::GitHubClient,
    index::SearchIndex,
    model::{
        ProfileDiscoveryRequest, ProfileSignal, RepositoryRecord, RepositoryResult,
        RepositorySource, Routing, SearchMode, SearchRequest, SearchResponse,
        StarredRepositoryProfile, SyncReport,
    },
    router::route_intent,
    storage::{RepositoryStore, StarredSort, normalize_starred_sort},
};

pub struct GitSeek {
    config: Config,
    store: RepositoryStore,
    index: SearchIndex,
    github: GitHubClient,
}

impl GitSeek {
    pub fn open(config: Config) -> Result<Self> {
        let store = RepositoryStore::open(&config.database_path())?;
        let index = SearchIndex::open_or_create(&config.index_dir())?;
        let github = GitHubClient::new(config.github_token.clone())?;
        Ok(Self {
            config,
            store,
            index,
            github,
        })
    }

    pub fn doctor(&self) -> serde_json::Value {
        serde_json::json!({
            "github_token": if self.github.has_token() { "present" } else { "missing" },
            "database_path": self.config.database_path(),
            "index_dir": self.config.index_dir(),
            "default_limit": self.config.default_limit,
            "include_readme": self.config.include_readme,
            "hints": doctor_hints(self.github.has_token()),
        })
    }

    pub async fn sync_starred(
        &self,
        force: bool,
        include_readme: Option<bool>,
        limit: Option<usize>,
    ) -> Result<SyncReport> {
        let include_readme = include_readme.unwrap_or(self.config.include_readme);
        let repos = self
            .github
            .starred_repositories(include_readme || force, limit)
            .await?;
        let mut updated_count = 0;
        for repo in &repos {
            if self.store.upsert_repository(repo)? {
                updated_count += 1;
            }
        }
        let starred = self.store.all_starred()?;
        self.index.rebuild(&starred)?;
        Ok(SyncReport {
            synced_count: repos.len(),
            updated_count,
            removed_count: 0,
            errors: Vec::new(),
        })
    }

    pub async fn search_starred(&self, request: SearchRequest) -> Result<SearchResponse> {
        let limit = request.limit_or(self.config.default_limit);
        let routing = Routing {
            mode: SearchMode::StarredOnly,
            reason: "Explicit starred search tool was called".to_string(),
        };
        let matches = self.search_starred_matches(&request, limit)?;
        Ok(SearchResponse::Single { routing, matches })
    }

    pub async fn search_github(&self, request: SearchRequest) -> Result<SearchResponse> {
        let limit = request.limit_or(self.config.default_limit);
        let routing = Routing {
            mode: SearchMode::GithubOnly,
            reason: "Explicit GitHub-wide search tool was called".to_string(),
        };
        let matches = self.search_github_matches(&request, limit).await?;
        Ok(SearchResponse::Single { routing, matches })
    }

    pub async fn recommend(
        &self,
        request: SearchRequest,
        prefer_starred: bool,
    ) -> Result<SearchResponse> {
        if !prefer_starred {
            return self.search_github(request).await;
        }

        let limit = request.limit_or(self.config.default_limit);
        let starred_target = limit;
        let starred_matches = self.search_starred_matches(&request, starred_target)?;
        let min_github_results = request.min_github_results.unwrap_or_else(|| {
            if starred_matches.len() >= limit {
                0
            } else {
                limit - starred_matches.len()
            }
        });
        let github_limit = min_github_results.max(limit.saturating_sub(starred_matches.len()));
        let github_matches = if github_limit == 0 {
            Vec::new()
        } else {
            self.search_github_matches(&request, github_limit).await?
        };
        let summary = format!(
            "Found {} starred matches first and {} GitHub-wide supplement matches.",
            starred_matches.len(),
            github_matches.len()
        );
        Ok(SearchResponse::Grouped {
            routing: Routing {
                mode: SearchMode::StarredFirstThenGithub,
                reason: "Recommendation tool preserves starred-first grouping".to_string(),
            },
            starred_matches,
            github_matches,
            summary,
        })
    }

    pub async fn discover_from_starred_profile(
        &self,
        request: ProfileDiscoveryRequest,
    ) -> Result<SearchResponse> {
        let starred = self.store.all_starred()?;
        if starred.is_empty() {
            bail!(
                "local starred repository index is empty; run sync_starred_repositories or gitseek sync stars first"
            );
        }

        let profile = build_starred_profile(&starred, &request);
        let limit = request.limit_or(self.config.default_limit);
        let mut excluded: HashSet<String> = starred
            .iter()
            .map(|repo| repo.full_name.to_ascii_lowercase())
            .collect();
        excluded.extend(
            request
                .exclude_full_names
                .iter()
                .map(|name| name.to_ascii_lowercase()),
        );

        let language = profile
            .top_languages
            .first()
            .map(|signal| signal.value.clone());
        let github_request = SearchRequest {
            query: profile.github_query.clone(),
            language,
            topics: Vec::new(),
            owner: None,
            limit: Some((limit * 3).min(50)),
            sort: Some("stars".to_string()),
            min_stars: Some(profile.min_stars),
            updated_after: None,
            min_github_results: None,
        };

        let mut github_matches = self
            .search_github_matches(&github_request, github_request.limit_or(50))
            .await?
            .into_iter()
            .filter(|result| !excluded.contains(&result.full_name.to_ascii_lowercase()))
            .take(limit)
            .collect::<Vec<_>>();

        for result in &mut github_matches {
            result.source = RepositorySource::Github;
            result.why_matched = profile_why_matched(&profile, result);
            result.recommended_use = Some(
                "High-star GitHub discovery based on the user's starred repository profile"
                    .to_string(),
            );
        }

        let summary = format!(
            "Built a profile from {} starred repositories and found {} high-star GitHub candidates with at least {} stars.",
            profile.starred_count,
            github_matches.len(),
            profile.min_stars
        );

        Ok(SearchResponse::ProfileDiscovery {
            routing: Routing {
                mode: SearchMode::StarredProfileGithubDiscovery,
                reason:
                    "Agent requested GitHub-wide discovery based on the user's starred repositories"
                        .to_string(),
            },
            profile,
            github_matches,
            summary,
        })
    }

    pub async fn route_and_search(&self, request: SearchRequest) -> Result<SearchResponse> {
        let routing = route_intent(&request.query);
        match routing.mode {
            SearchMode::StarredOnly => self.search_starred(request).await,
            SearchMode::GithubOnly => self.search_github(request).await,
            SearchMode::StarredFirstThenGithub => self.recommend(request, true).await,
            SearchMode::StarredProfileGithubDiscovery => {
                self.discover_from_starred_profile(ProfileDiscoveryRequest {
                    limit: request.limit,
                    min_stars: request.min_stars,
                    top_languages: None,
                    top_topics: None,
                    include_languages: request.language.into_iter().collect(),
                    include_topics: request.topics,
                    exclude_full_names: Vec::new(),
                })
                .await
            }
        }
    }

    fn search_starred_matches(
        &self,
        request: &SearchRequest,
        limit: usize,
    ) -> Result<Vec<RepositoryResult>> {
        let sort = normalize_starred_sort(request.sort.as_deref());
        let repos = if sort == StarredSort::Relevance {
            let full_names = self
                .index
                .search_full_names(&request.query, limit)
                .unwrap_or_default();
            if full_names.is_empty() {
                self.store
                    .text_search_starred(&request.query, limit, request.sort.as_deref())?
            } else {
                self.store.find_by_full_names(&full_names)?
            }
        } else {
            self.store
                .text_search_starred(&request.query, limit, request.sort.as_deref())?
        };

        let mut repos = repos
            .into_iter()
            .filter(|repo| {
                request.language.as_ref().is_none_or(|language| {
                    repo.language
                        .as_deref()
                        .is_some_and(|repo_language| repo_language.eq_ignore_ascii_case(language))
                })
            })
            .filter(|repo| {
                request
                    .owner
                    .as_ref()
                    .is_none_or(|owner| repo.owner.eq_ignore_ascii_case(owner))
            })
            .filter(|repo| {
                request.topics.iter().all(|topic| {
                    repo.topics
                        .iter()
                        .any(|repo_topic| repo_topic.eq_ignore_ascii_case(topic))
                })
            })
            .collect::<Vec<_>>();
        sort_starred_records(&mut repos, request.sort.as_deref());

        let matches = repos
            .into_iter()
            .take(limit)
            .map(|repo| {
                let mut result = repo.to_result(RepositorySource::Starred, false);
                result.why_matched = why_matched(&request.query, &result);
                result.recommended_use = Some("Use as a locally starred reference".to_string());
                result
            })
            .collect();
        Ok(matches)
    }

    async fn search_github_matches(
        &self,
        request: &SearchRequest,
        limit: usize,
    ) -> Result<Vec<RepositoryResult>> {
        let cache_key = github_cache_key(request, limit);
        if let Some((json, fetched_at)) = self.store.cached_search(&cache_key)? {
            if Utc::now()
                .signed_duration_since(fetched_at)
                .to_std()
                .unwrap_or(Duration::MAX)
                <= self.config.github_cache_ttl
            {
                let mut results: Vec<RepositoryResult> = serde_json::from_str(&json)
                    .context("failed to decode cached GitHub search response")?;
                for result in &mut results {
                    result.source = RepositorySource::Github;
                    result.cache_hit = true;
                }
                return Ok(results);
            }
        }

        let mut results = self
            .github
            .search_repositories(
                &request.query,
                request.language.as_deref(),
                &request.topics,
                request.min_stars,
                request.updated_after.as_deref(),
                limit,
                request.sort.as_deref(),
            )
            .await?;
        for result in &mut results {
            result.source = RepositorySource::Github;
            result.cache_hit = false;
            result.why_matched = why_matched(&request.query, result);
            result.recommended_use = Some("Use as a GitHub-wide discovery candidate".to_string());
        }
        self.store
            .put_cached_search(&cache_key, &serde_json::to_string(&results)?)?;
        Ok(results)
    }
}

pub async fn repository_context(full_name: &str) -> Result<serde_json::Value> {
    if !full_name.contains('/') {
        bail!("full_name must use owner/repo format");
    }
    Ok(serde_json::json!({
        "full_name": full_name,
        "url": format!("https://github.com/{full_name}"),
        "links": {
            "clone_url": format!("https://github.com/{full_name}.git"),
            "issues_url": format!("https://github.com/{full_name}/issues"),
            "releases_url": format!("https://github.com/{full_name}/releases")
        }
    }))
}

fn why_matched(query: &str, result: &RepositoryResult) -> Vec<String> {
    let query = query.to_ascii_lowercase();
    let mut reasons = Vec::new();
    if result.full_name.to_ascii_lowercase().contains(&query) {
        reasons.push("Repository full_name matches the query".to_string());
    }
    if result
        .description
        .as_deref()
        .is_some_and(|description| description.to_ascii_lowercase().contains(&query))
    {
        reasons.push("Repository description matches the query".to_string());
    }
    if result
        .topics
        .iter()
        .any(|topic| query.contains(&topic.to_ascii_lowercase()))
    {
        reasons.push("Repository topics overlap with the query".to_string());
    }
    if reasons.is_empty() {
        reasons.push("Repository matched the selected search backend".to_string());
    }
    reasons
}

fn github_cache_key(request: &SearchRequest, limit: usize) -> String {
    serde_json::json!({
        "query": request.query,
        "language": request.language,
        "topics": request.topics,
        "min_stars": request.min_stars,
        "updated_after": request.updated_after,
        "sort": request.sort,
        "limit": limit,
    })
    .to_string()
}

fn doctor_hints(has_token: bool) -> Vec<&'static str> {
    if has_token {
        Vec::new()
    } else {
        vec!["Set GITHUB_TOKEN before sync or GitHub-wide search"]
    }
}

fn sort_starred_records(repos: &mut [RepositoryRecord], sort: Option<&str>) {
    match normalize_starred_sort(sort) {
        StarredSort::StarredAt => repos.sort_by(|left, right| {
            right
                .starred_at
                .cmp(&left.starred_at)
                .then_with(|| right.stars.cmp(&left.stars))
                .then_with(|| left.full_name.cmp(&right.full_name))
        }),
        StarredSort::Stars => repos.sort_by(|left, right| {
            right
                .stars
                .cmp(&left.stars)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
                .then_with(|| left.full_name.cmp(&right.full_name))
        }),
        StarredSort::Updated => repos.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.stars.cmp(&left.stars))
                .then_with(|| left.full_name.cmp(&right.full_name))
        }),
        StarredSort::Name => repos.sort_by_key(|repo| repo.full_name.to_ascii_lowercase()),
        StarredSort::Relevance => {}
    }
}

fn build_starred_profile(
    starred: &[RepositoryRecord],
    request: &ProfileDiscoveryRequest,
) -> StarredRepositoryProfile {
    let mut language_counts = count_languages(starred);
    let mut topic_counts = count_topics(starred);

    for language in &request.include_languages {
        language_counts
            .entry(language.to_string())
            .and_modify(|count| *count += starred.len())
            .or_insert(starred.len());
    }
    for topic in &request.include_topics {
        topic_counts
            .entry(topic.to_string())
            .and_modify(|count| *count += starred.len())
            .or_insert(starred.len());
    }

    let top_languages = top_signals(language_counts, request.top_languages_or_default());
    let top_topics = top_signals(topic_counts, request.top_topics_or_default());
    let mut query_terms = top_topics
        .iter()
        .take(5)
        .map(|signal| signal.value.clone())
        .collect::<Vec<_>>();
    if query_terms.is_empty() {
        query_terms.extend(
            top_languages
                .iter()
                .take(3)
                .map(|signal| signal.value.clone()),
        );
    }
    if query_terms.is_empty() {
        query_terms.push("developer tools".to_string());
    }

    StarredRepositoryProfile {
        starred_count: starred.len(),
        top_languages,
        top_topics,
        seed_repositories: starred
            .iter()
            .take(10)
            .map(|repo| repo.full_name.clone())
            .collect(),
        github_query: query_terms.join(" "),
        min_stars: request.min_stars_or_default(),
    }
}

fn count_languages(starred: &[RepositoryRecord]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for repo in starred {
        if let Some(language) = repo.language.as_deref().filter(|value| !value.is_empty()) {
            *counts.entry(language.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

fn count_topics(starred: &[RepositoryRecord]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for repo in starred {
        for topic in &repo.topics {
            if !topic.is_empty() {
                *counts.entry(topic.to_string()).or_insert(0) += 1;
            }
        }
    }
    counts
}

fn top_signals(counts: HashMap<String, usize>, limit: usize) -> Vec<ProfileSignal> {
    let mut signals = counts
        .into_iter()
        .map(|(value, count)| ProfileSignal { value, count })
        .collect::<Vec<_>>();
    signals.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.value.cmp(&right.value))
    });
    signals.truncate(limit);
    signals
}

fn profile_why_matched(
    profile: &StarredRepositoryProfile,
    result: &RepositoryResult,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if result.stars >= profile.min_stars {
        reasons.push(format!(
            "Repository has at least {} stars",
            profile.min_stars
        ));
    }
    if result.language.as_ref().is_some_and(|language| {
        profile
            .top_languages
            .iter()
            .any(|signal| signal.value.eq_ignore_ascii_case(language))
    }) {
        reasons.push("Repository language matches the starred profile".to_string());
    }
    let overlapping_topics = result
        .topics
        .iter()
        .filter(|topic| {
            profile
                .top_topics
                .iter()
                .any(|signal| signal.value.eq_ignore_ascii_case(topic))
        })
        .take(3)
        .cloned()
        .collect::<Vec<_>>();
    if !overlapping_topics.is_empty() {
        reasons.push(format!(
            "Repository topics overlap with starred profile: {}",
            overlapping_topics.join(", ")
        ));
    }
    if reasons.is_empty() {
        reasons.push("Repository matched the GitHub-wide starred-profile query".to_string());
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RepositorySource;

    #[test]
    fn grouped_response_preserves_sources_and_cache_as_separate_fields() {
        let starred = RepositoryResult {
            full_name: "owner/starred".to_string(),
            url: "https://github.com/owner/starred".to_string(),
            description: None,
            language: Some("Rust".to_string()),
            stars: 10,
            starred_at: None,
            topics: vec!["mcp".to_string()],
            source: RepositorySource::Starred,
            cache_hit: false,
            why_matched: vec!["test".to_string()],
            recommended_use: None,
        };
        let github = RepositoryResult {
            source: RepositorySource::Github,
            cache_hit: true,
            full_name: "owner/github".to_string(),
            url: "https://github.com/owner/github".to_string(),
            description: None,
            language: None,
            stars: 1,
            starred_at: None,
            topics: Vec::new(),
            why_matched: Vec::new(),
            recommended_use: None,
        };
        let response = SearchResponse::Grouped {
            routing: Routing {
                mode: SearchMode::StarredFirstThenGithub,
                reason: "test".to_string(),
            },
            starred_matches: vec![starred],
            github_matches: vec![github],
            summary: "test".to_string(),
        };

        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["starred_matches"][0]["source"], "starred");
        assert_eq!(json["github_matches"][0]["source"], "github");
        assert_eq!(json["github_matches"][0]["cache_hit"], true);
    }

    #[test]
    fn builds_starred_profile_from_languages_and_topics() {
        let repos = vec![
            fixture_repo(
                "modelcontextprotocol/rust-sdk",
                Some("Rust"),
                &["mcp", "rust", "agent"],
            ),
            fixture_repo(
                "rust-lang/rust-analyzer",
                Some("Rust"),
                &["rust", "developer-tools"],
            ),
            fixture_repo("vercel/ai", Some("TypeScript"), &["ai", "agent"]),
        ];
        let request = ProfileDiscoveryRequest {
            limit: Some(5),
            min_stars: Some(5_000),
            top_languages: Some(2),
            top_topics: Some(3),
            include_languages: Vec::new(),
            include_topics: Vec::new(),
            exclude_full_names: Vec::new(),
        };

        let profile = build_starred_profile(&repos, &request);

        assert_eq!(profile.starred_count, 3);
        assert_eq!(profile.top_languages[0].value, "Rust");
        assert_eq!(profile.top_languages[0].count, 2);
        assert!(profile.github_query.contains("agent") || profile.github_query.contains("rust"));
        assert_eq!(profile.min_stars, 5_000);
    }

    #[test]
    fn sorts_starred_records_by_starred_at_desc() {
        let mut repos = vec![
            fixture_repo_with_starred_at(
                "owner/older",
                Some("Rust"),
                &["rust"],
                "2024-01-01T00:00:00Z",
            ),
            fixture_repo_with_starred_at(
                "owner/newer",
                Some("Rust"),
                &["rust"],
                "2026-01-01T00:00:00Z",
            ),
            fixture_repo_with_starred_at(
                "owner/middle",
                Some("Rust"),
                &["rust"],
                "2025-01-01T00:00:00Z",
            ),
        ];

        sort_starred_records(&mut repos, Some("starred_at"));

        assert_eq!(
            repos
                .iter()
                .map(|repo| repo.full_name.as_str())
                .collect::<Vec<_>>(),
            vec!["owner/newer", "owner/middle", "owner/older"]
        );
    }

    fn fixture_repo(full_name: &str, language: Option<&str>, topics: &[&str]) -> RepositoryRecord {
        let (owner, name) = full_name.split_once('/').unwrap();
        RepositoryRecord {
            github_id: 1,
            owner: owner.to_string(),
            name: name.to_string(),
            full_name: full_name.to_string(),
            url: format!("https://github.com/{full_name}"),
            description: None,
            homepage: None,
            topics: topics.iter().map(|topic| (*topic).to_string()).collect(),
            language: language.map(str::to_string),
            license: None,
            stars: 1,
            forks: 0,
            watchers: 1,
            created_at: None,
            updated_at: None,
            pushed_at: None,
            starred_at: None,
            last_synced_at: None,
            readme_fetched_at: None,
            etag: None,
            source: RepositorySource::Starred,
            readme: None,
        }
    }

    fn fixture_repo_with_starred_at(
        full_name: &str,
        language: Option<&str>,
        topics: &[&str],
        starred_at: &str,
    ) -> RepositoryRecord {
        let mut repo = fixture_repo(full_name, language, topics);
        repo.starred_at = Some(
            chrono::DateTime::parse_from_rfc3339(starred_at)
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        repo
    }
}
