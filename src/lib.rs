pub mod cli;
pub mod config;
pub mod github;
pub mod index;
pub mod mcp;
pub mod model;
pub mod router;
pub mod service;
pub mod storage;

pub use config::Config;
pub use model::{
    ProfileDiscoveryRequest, RepositoryResult, RepositorySource, Routing, SearchMode,
    SearchRequest, SearchResponse, StarredRepositoryProfile,
};
pub use service::GitSeek;
