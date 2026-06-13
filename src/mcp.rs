use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, stdin, stdout};

use crate::{
    GitSeek,
    model::{ProfileDiscoveryRequest, SearchRequest},
    service::repository_context,
};

pub async fn serve(gitseek: GitSeek) -> Result<()> {
    let stdin = BufReader::new(stdin());
    let mut lines = stdin.lines();
    let mut stdout = stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let request: RpcRequest = serde_json::from_str(&line)
            .with_context(|| format!("failed to parse JSON-RPC request: {line}"))?;
        if let Some(response) = handle_request(&gitseek, request).await {
            stdout
                .write_all(serde_json::to_string(&response)?.as_bytes())
                .await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

async fn handle_request(gitseek: &GitSeek, request: RpcRequest) -> Option<Value> {
    let id = request.id.clone();
    let is_notification = id.is_none();
    if is_notification {
        return match request.method.as_str() {
            "notifications/initialized" | "notifications/cancelled" => None,
            _ => None,
        };
    }

    let result = match request.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "gitseek", "version": env!("CARGO_PKG_VERSION") }
        })),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call_tool(gitseek, request.params).await,
        _ => Err(format!("unknown method {}", request.method)),
    };

    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(message) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32000, "message": message }
        }),
    })
}

async fn call_tool(gitseek: &GitSeek, params: Option<Value>) -> std::result::Result<Value, String> {
    let call: ToolCall = serde_json::from_value(params.unwrap_or_default())
        .map_err(|error| format!("invalid tool call params: {error}"))?;
    let arguments = call.arguments.unwrap_or_default();
    let value: Value = match call.name.as_str() {
        "search_starred_repositories" => {
            let request: SearchRequest = serde_json::from_value(arguments)
                .map_err(|error| format!("invalid starred search arguments: {error}"))?;
            serde_json::to_value(
                gitseek
                    .search_starred(request)
                    .await
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?
        }
        "search_github_repositories" => {
            let request: SearchRequest = serde_json::from_value(arguments)
                .map_err(|error| format!("invalid GitHub search arguments: {error}"))?;
            serde_json::to_value(
                gitseek
                    .search_github(request)
                    .await
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?
        }
        "recommend_repositories" => {
            let input: RecommendInput = serde_json::from_value(arguments)
                .map_err(|error| format!("invalid recommendation arguments: {error}"))?;
            serde_json::to_value(
                gitseek
                    .recommend(input.request, input.prefer_starred.unwrap_or(true))
                    .await
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?
        }
        "sync_starred_repositories" => {
            let input: SyncInput = serde_json::from_value(arguments)
                .map_err(|error| format!("invalid sync arguments: {error}"))?;
            serde_json::to_value(
                gitseek
                    .sync_starred(
                        input.force.unwrap_or(false),
                        input.include_readme,
                        input.limit,
                    )
                    .await
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?
        }
        "discover_repositories_from_starred_profile" => {
            let request: ProfileDiscoveryRequest = serde_json::from_value(arguments)
                .map_err(|error| format!("invalid starred-profile discovery arguments: {error}"))?;
            serde_json::to_value(
                gitseek
                    .discover_from_starred_profile(request)
                    .await
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?
        }
        "get_repository_context" => {
            let input: ContextInput = serde_json::from_value(arguments)
                .map_err(|error| format!("invalid context arguments: {error}"))?;
            repository_context(&input.full_name)
                .await
                .map_err(|error| error.to_string())?
        }
        _ => return Err(format!("unknown tool {}", call.name)),
    };

    Ok(json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
        }],
        "structuredContent": value
    }))
}

fn tools() -> Vec<Value> {
    vec![
        tool(
            "search_starred_repositories",
            "Search only the local starred repository index.",
            search_schema(),
        ),
        tool(
            "search_github_repositories",
            "Search only GitHub-wide repositories.",
            search_schema(),
        ),
        tool(
            "recommend_repositories",
            "Search starred repositories first, then GitHub-wide repositories as grouped supplement.",
            recommend_schema(),
        ),
        tool(
            "sync_starred_repositories",
            "Synchronize authenticated user's starred repositories into the local index.",
            sync_schema(),
        ),
        tool(
            "discover_repositories_from_starred_profile",
            "Build an interest profile from local starred repositories, then recommend high-star GitHub-wide repositories the user may like.",
            profile_discovery_schema(),
        ),
        tool(
            "get_repository_context",
            "Return an agent-ready context packet for one repository.",
            context_schema(),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": input_schema })
}

fn search_schema() -> Value {
    json!({
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": { "type": "string" },
            "language": { "type": "string" },
            "topics": { "type": "array", "items": { "type": "string" } },
            "owner": { "type": "string" },
            "limit": { "type": "integer", "minimum": 1, "maximum": 50 },
            "sort": { "type": "string" },
            "min_stars": { "type": "integer", "minimum": 0 },
            "updated_after": { "type": "string" },
            "min_github_results": { "type": "integer", "minimum": 0 }
        }
    })
}

fn recommend_schema() -> Value {
    let mut schema = search_schema();
    schema["properties"]["prefer_starred"] = json!({ "type": "boolean", "default": true });
    schema
}

fn sync_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "force": { "type": "boolean" },
            "include_readme": { "type": "boolean", "default": true },
            "limit": { "type": "integer", "minimum": 1 }
        }
    })
}

fn profile_discovery_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 },
            "min_stars": { "type": "integer", "minimum": 0, "default": 1000 },
            "top_languages": { "type": "integer", "minimum": 1, "maximum": 10, "default": 3 },
            "top_topics": { "type": "integer", "minimum": 1, "maximum": 20, "default": 8 },
            "include_languages": { "type": "array", "items": { "type": "string" } },
            "include_topics": { "type": "array", "items": { "type": "string" } },
            "exclude_full_names": { "type": "array", "items": { "type": "string" } }
        }
    })
}

fn context_schema() -> Value {
    json!({
        "type": "object",
        "required": ["full_name"],
        "properties": {
            "full_name": { "type": "string" },
            "source_hint": { "type": "string", "enum": ["starred", "github"] },
            "include_readme": { "type": "boolean", "default": true }
        }
    })
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ToolCall {
    name: String,
    arguments: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct RecommendInput {
    #[serde(flatten)]
    request: SearchRequest,
    prefer_starred: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct SyncInput {
    force: Option<bool>,
    include_readme: Option<bool>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ContextInput {
    full_name: String,
}
