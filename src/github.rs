use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;

use crate::model::{RepositoryRecord, RepositoryResult, RepositorySource};

#[derive(Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    token: Option<String>,
}

impl GitHubClient {
    pub fn new(token: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .default_headers(default_headers(token.as_deref())?)
            .build()
            .context("failed to build GitHub HTTP client")?;
        Ok(Self { http, token })
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    pub async fn starred_repositories(
        &self,
        include_readme: bool,
        limit: Option<usize>,
    ) -> Result<Vec<RepositoryRecord>> {
        if self.token.is_none() {
            bail!("missing GitHub token; set GITHUB_TOKEN or run gitseek doctor");
        }

        let mut page = 1;
        let mut repos = Vec::new();
        loop {
            let url = format!(
                "https://api.github.com/user/starred?sort=created&direction=desc&per_page=100&page={page}"
            );
            let page_items: Vec<GitHubStarredRepo> = self
                .http
                .get(&url)
                .header(ACCEPT, "application/vnd.github.star+json")
                .send()
                .await
                .with_context(|| format!("failed to fetch starred repositories page {page}"))?
                .error_for_status()
                .context("GitHub starred repositories request failed")?
                .json()
                .await
                .context("failed to decode starred repositories response")?;

            if page_items.is_empty() {
                break;
            }

            for item in page_items {
                if limit.is_some_and(|limit| repos.len() >= limit) {
                    return Ok(repos);
                }
                let full_name = item.repo.full_name.clone();
                let mut record = item.into_record();
                if include_readme {
                    record.readme = self.readme_excerpt(&full_name).await.ok();
                    if record.readme.is_some() {
                        record.readme_fetched_at = Some(Utc::now());
                    }
                }
                repos.push(record);
            }
            page += 1;
        }
        Ok(repos)
    }

    pub async fn search_repositories(
        &self,
        query: &str,
        language: Option<&str>,
        topics: &[String],
        min_stars: Option<u64>,
        updated_after: Option<&str>,
        limit: usize,
        sort: Option<&str>,
    ) -> Result<Vec<RepositoryResult>> {
        let mut q = query.to_string();
        if let Some(language) = language {
            q.push_str(&format!(" language:{language}"));
        }
        for topic in topics {
            q.push_str(&format!(" topic:{topic}"));
        }
        if let Some(min_stars) = min_stars {
            q.push_str(&format!(" stars:>={min_stars}"));
        }
        if let Some(updated_after) = updated_after {
            q.push_str(&format!(" pushed:>={updated_after}"));
        }

        let sort = sort.unwrap_or("stars");
        let url = format!(
            "https://api.github.com/search/repositories?q={}&sort={}&per_page={}",
            urlencoding::encode(&q),
            urlencoding::encode(sort),
            limit.min(50)
        );

        let response: GitHubSearchResponse = self
            .http
            .get(&url)
            .send()
            .await
            .context("failed to search GitHub repositories")?
            .error_for_status()
            .context("GitHub repository search request failed")?
            .json()
            .await
            .context("failed to decode GitHub repository search response")?;

        Ok(response
            .items
            .into_iter()
            .map(|repo| repo.into_result(false))
            .collect())
    }

    async fn readme_excerpt(&self, full_name: &str) -> Result<String> {
        let url = format!("https://api.github.com/repos/{full_name}/readme");
        let response: GitHubReadme = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(response.content.chars().take(12_000).collect())
    }
}

fn default_headers(token: Option<&str>) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("gitseek/0.1.0"));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    if let Some(token) = token {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .context("failed to build GitHub authorization header")?;
        headers.insert(AUTHORIZATION, value);
    }
    Ok(headers)
}

#[derive(Debug, Deserialize)]
struct GitHubSearchResponse {
    items: Vec<GitHubRepo>,
}

#[derive(Debug, Deserialize)]
struct GitHubReadme {
    content: String,
}

#[derive(Debug, Deserialize)]
struct GitHubStarredRepo {
    starred_at: DateTime<Utc>,
    repo: GitHubRepo,
}

impl GitHubStarredRepo {
    fn into_record(self) -> RepositoryRecord {
        self.repo
            .into_record(RepositorySource::Starred, Some(self.starred_at))
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRepo {
    id: i64,
    name: String,
    full_name: String,
    html_url: String,
    description: Option<String>,
    homepage: Option<String>,
    topics: Option<Vec<String>>,
    language: Option<String>,
    license: Option<GitHubLicense>,
    stargazers_count: u64,
    forks_count: u64,
    watchers_count: u64,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    pushed_at: Option<DateTime<Utc>>,
    owner: GitHubOwner,
}

impl GitHubRepo {
    fn into_record(
        self,
        source: RepositorySource,
        starred_at: Option<DateTime<Utc>>,
    ) -> RepositoryRecord {
        RepositoryRecord {
            github_id: self.id,
            owner: self.owner.login,
            name: self.name,
            full_name: self.full_name,
            url: self.html_url,
            description: self.description,
            homepage: self.homepage,
            topics: self.topics.unwrap_or_default(),
            language: self.language,
            license: self.license.map(|license| license.spdx_id),
            stars: self.stargazers_count,
            forks: self.forks_count,
            watchers: self.watchers_count,
            created_at: self.created_at,
            updated_at: self.updated_at,
            pushed_at: self.pushed_at,
            starred_at,
            last_synced_at: Some(Utc::now()),
            readme_fetched_at: None,
            etag: None,
            source,
            readme: None,
        }
    }

    fn into_result(self, cache_hit: bool) -> RepositoryResult {
        self.into_record(RepositorySource::Github, None)
            .to_result(RepositorySource::Github, cache_hit)
    }
}

#[derive(Debug, Deserialize)]
struct GitHubOwner {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GitHubLicense {
    spdx_id: String,
}
