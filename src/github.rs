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
    api_base: String,
}

impl GitHubClient {
    pub fn new(token: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .default_headers(default_headers(token.as_deref())?)
            .build()
            .context("failed to build GitHub HTTP client")?;
        Ok(Self {
            http,
            token,
            api_base: "https://api.github.com".into(),
        })
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
                "{}/user/starred?sort=created&direction=desc&per_page=100&page={page}",
                self.api_base
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
            "{}/search/repositories?q={}&sort={}&per_page={}",
            self.api_base,
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
        let url = format!("{}/repos/{full_name}/readme", self.api_base);
        let response = self
            .http
            .get(url)
            .header(ACCEPT, "application/vnd.github.raw+json")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(response.chars().take(12_000).collect())
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    pub(crate) fn mock_github(
        responses: Vec<(u16, String)>,
    ) -> (GitHubClient, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api_base = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    request.push_str(&line);
                }
                requests.push(request);
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        let http = reqwest::Client::builder()
            .no_proxy()
            .default_headers(default_headers(Some("test-token")).unwrap())
            .build()
            .unwrap();
        (
            GitHubClient {
                http,
                token: Some("test-token".into()),
                api_base,
            },
            server,
        )
    }

    pub(crate) fn starred(id: i64, name: &str) -> serde_json::Value {
        json!({"starred_at":"2026-10-01T00:00:00Z", "repo":{
            "id":id,"name":name,"full_name":format!("owner/{name}"),
            "html_url":format!("https://github.com/owner/{name}"),"owner":{"login":"owner"},
            "stargazers_count":1,"forks_count":0,"watchers_count":1,
        }})
    }

    #[tokio::test]
    async fn readme_requests_raw_text_and_truncates_unicode_characters() {
        let readme = format!("# Actual README\n{}END", "中文🚀".repeat(5_000));
        let (client, server) = mock_github(vec![(200, readme.clone())]);
        let excerpt = client.readme_excerpt("owner/tool").await.unwrap();
        assert_eq!(excerpt, readme.chars().take(12_000).collect::<String>());
        assert_eq!(excerpt.chars().count(), 12_000);
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("GET /repos/owner/tool/readme "));
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("accept: application/vnd.github.raw+json\r\n")
        );
    }
}
