use std::{env, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Config {
    pub github_token: Option<String>,
    pub data_dir: PathBuf,
    pub default_limit: usize,
    pub github_cache_ttl: Duration,
    pub include_readme: bool,
}

#[derive(Debug, Deserialize)]
struct FileConfig {
    github: Option<GithubConfig>,
    storage: Option<StorageConfig>,
    search: Option<SearchConfig>,
    sync: Option<SyncConfig>,
}

#[derive(Debug, Deserialize)]
struct GithubConfig {
    token_env: Option<String>,
    cache_ttl_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct StorageConfig {
    data_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct SearchConfig {
    default_limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SyncConfig {
    include_readme: Option<bool>,
}

impl Config {
    pub fn load() -> Result<Self> {
        if let Ok(path) = env::var("GITSEEK_ENV_FILE") {
            let _ = dotenvy::from_path(path);
        } else {
            let _ = dotenvy::dotenv();
        }

        let path = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gitseek")
            .join("config.toml");

        let file_config =
            if path.exists() {
                let raw = std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read config at {}", path.display()))?;
                Some(toml::from_str::<FileConfig>(&raw).with_context(|| {
                    format!("failed to parse config TOML at {}", path.display())
                })?)
            } else {
                None
            };

        let token_env = file_config
            .as_ref()
            .and_then(|config| config.github.as_ref())
            .and_then(|github| github.token_env.as_deref())
            .unwrap_or("GITHUB_TOKEN");

        let data_dir = file_config
            .as_ref()
            .and_then(|config| config.storage.as_ref())
            .and_then(|storage| storage.data_dir.clone())
            .or_else(|| dirs::data_dir().map(|dir| dir.join("gitseek")))
            .unwrap_or_else(|| PathBuf::from(".gitseek"));

        Ok(Self {
            github_token: env::var(token_env).ok(),
            data_dir,
            default_limit: file_config
                .as_ref()
                .and_then(|config| config.search.as_ref())
                .and_then(|search| search.default_limit)
                .unwrap_or(10),
            github_cache_ttl: Duration::from_secs(
                file_config
                    .as_ref()
                    .and_then(|config| config.github.as_ref())
                    .and_then(|github| github.cache_ttl_seconds)
                    .unwrap_or(3600),
            ),
            include_readme: file_config
                .as_ref()
                .and_then(|config| config.sync.as_ref())
                .and_then(|sync| sync.include_readme)
                .unwrap_or(true),
        })
    }

    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("gitseek.sqlite3")
    }

    #[must_use]
    pub fn index_dir(&self) -> PathBuf {
        self.data_dir.join("tantivy")
    }
}
