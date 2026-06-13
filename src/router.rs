use crate::model::{Routing, SearchMode};

#[must_use]
pub fn route_intent(intent: &str) -> Routing {
    let normalized = intent.to_ascii_lowercase();

    if contains_any(
        &normalized,
        &[
            "my starred",
            "starred repos",
            "starred repositories",
            "search stars",
            "search in my stars",
            "我 star",
            "我的 star",
            "星标中",
            "星标里",
            "只搜 starred",
            "只搜星标",
        ],
    ) && !contains_any(&normalized, &["prefer", "优先", "first"])
    {
        return Routing {
            mode: SearchMode::StarredOnly,
            reason: "User asked to search only starred repositories".to_string(),
        };
    }

    if contains_any(
        &normalized,
        &[
            "search github",
            "all github",
            "github-wide",
            "github 上",
            "github 全站",
            "全站",
        ],
    ) && !contains_any(&normalized, &["prefer", "优先", "first"])
    {
        return Routing {
            mode: SearchMode::GithubOnly,
            reason: "User asked to search GitHub-wide repositories".to_string(),
        };
    }

    Routing {
        mode: SearchMode::StarredFirstThenGithub,
        reason: if contains_any(&normalized, &["prefer", "优先", "first", "先搜"]) {
            "User asked to prioritize starred repositories".to_string()
        } else {
            "Unclear scope defaults to personal stars first, then GitHub-wide discovery".to_string()
        },
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_starred_only_phrases() {
        assert_eq!(
            route_intent("search in my starred repos for rust mcp").mode,
            SearchMode::StarredOnly
        );
        assert_eq!(
            route_intent("在星标中搜索 rust mcp").mode,
            SearchMode::StarredOnly
        );
    }

    #[test]
    fn routes_github_only_phrases() {
        assert_eq!(
            route_intent("search GitHub for rust mcp").mode,
            SearchMode::GithubOnly
        );
        assert_eq!(
            route_intent("在 GitHub 全站搜索 rust mcp").mode,
            SearchMode::GithubOnly
        );
    }

    #[test]
    fn routes_starred_first_phrases_and_unclear_scope() {
        assert_eq!(
            route_intent("prefer my stars, then search GitHub").mode,
            SearchMode::StarredFirstThenGithub
        );
        assert_eq!(
            route_intent("find rust mcp examples").mode,
            SearchMode::StarredFirstThenGithub
        );
    }
}
