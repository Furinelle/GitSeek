use std::{
    io::Write,
    process::{Command, Stdio},
    time::Duration,
};

use gitseek::{
    Config,
    model::{RepositoryRecord, RepositorySource},
    storage::RepositoryStore,
};
use serde_json::{Value, json};

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: temp.path().to_path_buf(),
        github_token: None,
        default_limit: 10,
        github_cache_ttl: Duration::from_secs(60),
        include_readme: false,
    };
    let store = RepositoryStore::open(&config.database_path()).unwrap();
    store
        .upsert_repository(&RepositoryRecord {
            github_id: 1,
            owner: "tools".into(),
            name: "telegram-search".into(),
            full_name: "tools/telegram-search".into(),
            url: "https://github.com/tools/telegram-search".into(),
            description: Some("Telegram folder search".into()),
            language: Some("Rust".into()),
            topics: vec!["telegram".into()],
            stars: 1,
            forks: 0,
            watchers: 0,
            homepage: None,
            license: None,
            created_at: None,
            updated_at: None,
            pushed_at: None,
            starred_at: None,
            last_synced_at: None,
            readme_fetched_at: None,
            etag: None,
            source: RepositorySource::Starred,
            readme: None,
        })
        .unwrap();
    std::fs::write(temp.path().join("empty.env"), "").unwrap();
    temp
}

fn command(temp: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gitseek"));
    command
        .args(["--data-dir", temp.path().to_str().unwrap()])
        .env("GITSEEK_ENV_FILE", temp.path().join("empty.env"))
        .env_remove("GITHUB_TOKEN");
    command
}

#[test]
fn cli_ignores_stale_tantivy_snapshot_after_sqlite_changes() {
    let temp = fixture();
    let store = RepositoryStore::open(&temp.path().join("gitseek.sqlite3")).unwrap();
    let old = store.all_starred().unwrap();
    let index = gitseek::index::SearchIndex::open_or_create(&temp.path().join("tantivy")).unwrap();
    index.rebuild(&old).unwrap();
    let mut replacement = old[0].clone();
    replacement.github_id = 2;
    replacement.full_name = "tools/archive".into();
    replacement.name = "archive".into();
    replacement.description = Some("Archive tools".into());
    replacement.topics = vec!["archive".into()];
    store.apply_starred_snapshot(&[replacement], true).unwrap();
    for (args, expected) in [
        (vec!["search", "stars", "telegram"], 0),
        (vec!["search", "stars", "archive"], 1),
        (vec!["discover", "for-project", "telegram"], 0),
    ] {
        let output = command(&temp).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["matches"].as_array().unwrap().len(), expected);
    }
}

#[test]
fn cli_reads_explicit_context_and_searches_without_a_tantivy_rebuild() {
    let temp = fixture();
    let context = temp.path().join("project.json");
    std::fs::write(&context, json!({
        "full_name": "user/archive", "description": "Telegram folder archive", "language": "Rust"
    }).to_string()).unwrap();
    let output = command(&temp)
        .args([
            "discover",
            "for-project",
            "user/archive",
            "--context",
            context.to_str().unwrap(),
            "--limit",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["matches"][0]["full_name"], "tools/telegram-search");
    assert_eq!(result["matches"][0]["source"], "starred");
    assert!(
        result["matches"][0]["why_matched"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    assert!(
        !result["query_terms"]
            .as_array()
            .unwrap()
            .contains(&json!("rust"))
    );
}

#[test]
fn mcp_advertises_and_calls_project_discovery_with_used_repository_exclusion() {
    let temp = fixture();
    let mut process = command(&temp)
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"discover_starred_for_project","arguments":{"target":"telegram search"}
        }}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
            "name":"discover_starred_for_project","arguments":{
                "target":"user/archive","context":{"full_name":"user/archive","description":"Telegram search", "used_repositories":["TOOLS/TELEGRAM-SEARCH"]}
            }
        }}),
    ];
    {
        let mut stdin = process.stdin.take().unwrap();
        for request in requests {
            writeln!(stdin, "{request}").unwrap();
        }
    }
    let output = process.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let responses: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let tool = responses[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "discover_starred_for_project")
        .unwrap();
    assert_eq!(tool["inputSchema"]["required"], json!(["target"]));
    assert_eq!(
        responses[1]["result"]["structuredContent"]["matches"][0]["source"],
        "starred"
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["excluded_count"],
        1
    );
    assert_eq!(
        responses[2]["result"]["structuredContent"]["matches"],
        json!([])
    );
}
