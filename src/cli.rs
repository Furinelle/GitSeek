use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

use crate::{
    Config, GitSeek,
    model::{ProfileDiscoveryRequest, ProjectContext, ProjectFitRequest, SearchRequest},
    service::repository_context,
};

#[derive(Debug, Parser)]
#[command(
    name = "gitseek",
    version,
    about = "Agent-first GitHub discovery MCP server"
)]
pub struct Cli {
    /// Override local storage, for example for an isolated offline evaluation.
    #[arg(long, global = true, env = "GITSEEK_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
    Search {
        #[command(subcommand)]
        command: SearchCommand,
    },
    Recommend(QueryArgs),
    Discover {
        #[command(subcommand)]
        command: DiscoverCommand,
    },
    Context {
        full_name: String,
    },
    Doctor,
}

#[derive(Debug, Subcommand)]
enum SyncCommand {
    Stars(SyncStarsArgs),
}

#[derive(Debug, Subcommand)]
enum SearchCommand {
    Stars(QueryArgs),
    Github(QueryArgs),
}

#[derive(Debug, Subcommand)]
enum DiscoverCommand {
    FromStars(ProfileDiscoveryArgs),
    /// Rank locally starred tools for a project or a short description of a need.
    ForProject(ProjectFitArgs),
}

#[derive(Debug, Args)]
struct ProjectFitArgs {
    target: String,
    /// JSON project metadata; GitSeek reads only this file, never the project directory.
    #[arg(long)]
    context: Option<PathBuf>,
    #[arg(long = "exclude")]
    exclude_full_names: Vec<String>,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Args)]
struct SyncStarsArgs {
    #[arg(long)]
    force: bool,
    #[arg(long)]
    no_readme: bool,
    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Args)]
pub struct QueryArgs {
    query: String,
    #[arg(long)]
    language: Option<String>,
    #[arg(long = "topic")]
    topics: Vec<String>,
    #[arg(long)]
    owner: Option<String>,
    #[arg(long)]
    limit: Option<usize>,
    #[arg(long)]
    sort: Option<String>,
    #[arg(long)]
    min_stars: Option<u64>,
    #[arg(long)]
    updated_after: Option<String>,
    #[arg(long)]
    min_github_results: Option<usize>,
}

#[derive(Debug, Args)]
pub struct ProfileDiscoveryArgs {
    #[arg(long)]
    limit: Option<usize>,
    #[arg(long)]
    min_stars: Option<u64>,
    #[arg(long)]
    top_languages: Option<usize>,
    #[arg(long)]
    top_topics: Option<usize>,
    #[arg(long = "include-language")]
    include_languages: Vec<String>,
    #[arg(long = "include-topic")]
    include_topics: Vec<String>,
    #[arg(long = "exclude")]
    exclude_full_names: Vec<String>,
}

impl From<QueryArgs> for SearchRequest {
    fn from(args: QueryArgs) -> Self {
        Self {
            query: args.query,
            language: args.language,
            topics: args.topics,
            owner: args.owner,
            limit: args.limit,
            sort: args.sort,
            min_stars: args.min_stars,
            updated_after: args.updated_after,
            min_github_results: args.min_github_results,
        }
    }
}

impl From<ProfileDiscoveryArgs> for ProfileDiscoveryRequest {
    fn from(args: ProfileDiscoveryArgs) -> Self {
        Self {
            limit: args.limit,
            min_stars: args.min_stars,
            top_languages: args.top_languages,
            top_topics: args.top_topics,
            include_languages: args.include_languages,
            include_topics: args.include_topics,
            exclude_full_names: args.exclude_full_names,
        }
    }
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    let mut config = Config::load()?;
    if let Some(data_dir) = cli.data_dir {
        config.data_dir = data_dir;
    }
    let gitseek = GitSeek::open(config)?;

    match cli.command {
        Command::Serve => crate::mcp::serve(gitseek).await,
        Command::Sync { command } => match command {
            SyncCommand::Stars(args) => {
                let report = gitseek
                    .sync_starred(args.force, Some(!args.no_readme), args.limit)
                    .await?;
                print_json(&report)
            }
        },
        Command::Search { command } => match command {
            SearchCommand::Stars(args) => {
                let response = gitseek.search_starred(args.into()).await?;
                print_json(&response)
            }
            SearchCommand::Github(args) => {
                let response = gitseek.search_github(args.into()).await?;
                print_json(&response)
            }
        },
        Command::Recommend(args) => {
            let response = gitseek.recommend(args.into(), true).await?;
            print_json(&response)
        }
        Command::Discover { command } => match command {
            DiscoverCommand::FromStars(args) => {
                let response = gitseek.discover_from_starred_profile(args.into()).await?;
                print_json(&response)
            }
            DiscoverCommand::ForProject(args) => {
                let context = args
                    .context
                    .map(|path| -> Result<ProjectContext> {
                        let raw = std::fs::read_to_string(&path).with_context(|| {
                            format!("failed to read project context {}", path.display())
                        })?;
                        serde_json::from_str(&raw).with_context(|| {
                            format!("invalid project context JSON {}", path.display())
                        })
                    })
                    .transpose()?;
                let response = gitseek.discover_starred_for_project(ProjectFitRequest {
                    target: args.target,
                    context,
                    exclude_full_names: args.exclude_full_names,
                    limit: args.limit,
                })?;
                print_json(&response)
            }
        },
        Command::Context { full_name } => {
            let context = repository_context(&full_name).await?;
            print_json(&context)
        }
        Command::Doctor => print_json(&gitseek.doctor()),
    }
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
