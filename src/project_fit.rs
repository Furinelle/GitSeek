use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::{Result, bail};

use crate::model::{
    ProjectContext, ProjectFitMatch, ProjectFitRequest, ProjectFitResponse, RepositoryRecord,
    RepositorySource,
};

pub(crate) fn discover(
    starred: &[RepositoryRecord],
    request: ProjectFitRequest,
    default_limit: usize,
) -> Result<ProjectFitResponse> {
    let target = request.target.trim();
    if target.is_empty() {
        bail!("target must be a repository name or a short description of the need");
    }
    if starred.is_empty() {
        bail!("local starred repository index is empty; run gitseek sync stars first");
    }
    let is_repository = repository_name(target);
    let context = match request.context {
        Some(context) => {
            if !repository_name(&context.full_name) {
                bail!("context.full_name must use owner/repo format");
            }
            if is_repository && !context.full_name.eq_ignore_ascii_case(target) {
                bail!("context.full_name must match the target repository");
            }
            Some(context)
        }
        None if is_repository => {
            let repo = starred
                .iter()
                .find(|repo| repo.full_name.eq_ignore_ascii_case(target));
            let Some(repo) = repo else {
                bail!(
                    "target repository is not in the local starred index; provide project context or describe the need instead"
                );
            };
            Some(ProjectContext {
                full_name: repo.full_name.clone(),
                description: repo.description.clone(),
                language: repo.language.clone(),
                topics: repo.topics.clone(),
                used_repositories: Vec::new(),
            })
        }
        None => None,
    };

    let mut terms = if is_repository {
        BTreeSet::new()
    } else {
        words(target)
    };
    let mut purpose_terms = terms.clone();
    let mut excluded: HashSet<String> = request
        .exclude_full_names
        .iter()
        .map(|name| name.trim().to_lowercase())
        .collect();
    if let Some(context) = &context {
        terms.extend(words(
            context.full_name.rsplit('/').next().unwrap_or_default(),
        ));
        terms.extend(description_words(
            context.description.as_deref().unwrap_or_default(),
        ));
        purpose_terms.extend(description_words(
            context
                .description
                .as_deref()
                .unwrap_or_default()
                .split(['.', '。', '\n'])
                .next()
                .unwrap_or_default(),
        ));
        for topic in &context.topics {
            terms.extend(words(topic));
            purpose_terms.extend(words(topic));
        }
        if let Some(language) = &context.language {
            for term in words(language) {
                terms.remove(&term);
                purpose_terms.remove(&term);
            }
        }
        excluded.insert(context.full_name.to_lowercase());
        excluded.extend(
            context
                .used_repositories
                .iter()
                .map(|name| name.trim().to_lowercase()),
        );
    }
    if terms.is_empty() {
        bail!("target has no searchable terms; provide a more specific need or project context");
    }

    let candidates = starred
        .iter()
        .filter(|repo| !excluded.contains(&repo.full_name.to_lowercase()))
        .collect::<Vec<_>>();
    let excluded_count = starred.len() - candidates.len();
    let fields = candidates
        .iter()
        .map(|repo| repository_fields(repo))
        .collect::<Vec<_>>();
    let mut document_frequency: HashMap<&str, u64> = HashMap::new();
    for fields in &fields {
        for term in &terms {
            if fields.iter().any(|(_, _, values)| values.contains(term)) {
                *document_frequency.entry(term).or_default() += 1;
            }
        }
    }

    let mut matches = Vec::new();
    for (repo, fields) in candidates.iter().zip(fields) {
        let mut score = 0;
        let mut reasons = Vec::new();
        let mut matched_purpose = BTreeSet::new();
        for (field, weight, values) in fields {
            let overlap = terms.intersection(&values).collect::<Vec<_>>();
            if !overlap.is_empty() {
                for term in &overlap {
                    // Rarer terms carry more evidence; popularity never enters the score.
                    let purpose_weight = if purpose_terms.contains(*term) {
                        matched_purpose.insert((*term).clone());
                        3
                    } else {
                        1
                    };
                    score += purpose_weight
                        * weight
                        * (1_000 / (1 + document_frequency[term.as_str()])).max(1);
                }
                reasons.push(format!(
                    "{field} contains: {}",
                    overlap
                        .iter()
                        .map(|term| term.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        // Language alone does not establish that a repository solves the project's need.
        if score == 0 {
            continue;
        }
        score *= matched_purpose.len().max(1) as u64;
        if let Some(language) = context
            .as_ref()
            .and_then(|context| context.language.as_ref())
            && repo
                .language
                .as_ref()
                .is_some_and(|value| value.eq_ignore_ascii_case(language))
        {
            score += 1;
            reasons.push(format!(
                "Same implementation language: {language} (supporting evidence only)"
            ));
        }
        let mut repository = repo.to_result(RepositorySource::Starred, false);
        repository.why_matched = reasons;
        repository.recommended_use = Some(format!("Evaluate as a reference or tool for {target}"));
        matches.push(ProjectFitMatch {
            relevance_score: score,
            repository,
        });
    }
    matches.sort_by(|left, right| {
        right
            .relevance_score
            .cmp(&left.relevance_score)
            .then_with(|| left.repository.full_name.cmp(&right.repository.full_name))
    });
    matches.truncate(request.limit.unwrap_or(default_limit).clamp(1, 50));

    Ok(ProjectFitResponse {
        target: target.to_string(),
        context,
        query_terms: terms.into_iter().collect(),
        purpose_terms: purpose_terms.into_iter().collect(),
        starred_count: starred.len(),
        excluded_count,
        matches,
        limitation: "Local lexical matches in repository name, topics, description and cached README; scores are relative evidence, not compatibility or quality ratings. Explicit English negative requirement sentences are omitted from project descriptions, but general semantic negation and synonym translation are not supported. No GitHub requests or LLM calls. Already-used repositories are excluded only when supplied in context.used_repositories or exclude_full_names.".to_string(),
    })
}

fn repository_name(value: &str) -> bool {
    let parts = value.split('/').collect::<Vec<_>>();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
}

fn repository_fields(repo: &RepositoryRecord) -> [(&'static str, u64, BTreeSet<String>); 4] {
    [
        ("Repository name", 3, words(&repo.name)),
        ("Topics", 4, words(&repo.topics.join(" "))),
        (
            "Description",
            2,
            words(repo.description.as_deref().unwrap_or_default()),
        ),
        (
            "Cached README",
            1,
            words(repo.readme.as_deref().unwrap_or_default()),
        ),
    ]
}

fn description_words(description: &str) -> BTreeSet<String> {
    // shortcut: only explicit English negative requirements, extend if broader negation is needed.
    description
        .split(['.', '!', '?', ';', '\n', '。', '！', '？', '；'])
        .filter(|sentence| {
            let sentence = sentence.to_lowercase();
            ![
                "do not require",
                "does not require",
                "don't require",
                "doesn't require",
                "do not use",
                "does not use",
                "without requiring",
            ]
            .iter()
            .any(|phrase| sentence.contains(phrase))
        })
        .flat_map(words)
        .collect()
}

fn words(value: &str) -> BTreeSet<String> {
    value
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() > 1)
        .filter(|word| {
            !matches!(
                *word,
                "the"
                    | "and"
                    | "for"
                    | "with"
                    | "from"
                    | "this"
                    | "that"
                    | "your"
                    | "our"
                    | "are"
                    | "was"
                    | "not"
                    | "into"
                    | "can"
                    | "use"
                    | "using"
                    | "used"
                    | "uses"
                    | "find"
                    | "need"
                    | "want"
                    | "project"
                    | "repository"
                    | "repo"
                    | "written"
                    | "built"
                    | "build"
                    | "powered"
                    | "based"
                    | "via"
                    | "in"
                    | "on"
                    | "to"
                    | "of"
                    | "an"
                    | "as"
                    | "is"
                    | "it"
                    | "be"
                    | "at"
                    | "by"
                    | "or"
                    | "all"
                    | "io"
            )
        })
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str, description: &str, language: &str, stars: u64) -> RepositoryRecord {
        RepositoryRecord {
            github_id: 1,
            owner: "owner".into(),
            name: name.into(),
            full_name: format!("owner/{name}"),
            url: format!("https://github.com/owner/{name}"),
            description: Some(description.into()),
            language: Some(language.into()),
            topics: Vec::new(),
            stars,
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
        }
    }

    fn request(target: &str) -> ProjectFitRequest {
        ProjectFitRequest {
            target: target.into(),
            context: None,
            exclude_full_names: Vec::new(),
            limit: None,
        }
    }

    #[test]
    fn relevance_uses_specific_evidence_and_never_star_popularity() {
        let repos = vec![
            repo("popular", "Telegram service", "Go", 1_000_000),
            repo("specific", "Telegram folder search", "Rust", 1),
            repo("unrelated", "File manager", "Rust", 2_000_000),
        ];
        let result = discover(&repos, request("telegram folder search"), 10).unwrap();
        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.matches[0].repository.full_name, "owner/specific");
        assert!(
            result.matches[0]
                .repository
                .why_matched
                .iter()
                .any(|reason| reason.contains("Description contains: folder, search, telegram"))
        );
        assert!(
            result
                .matches
                .iter()
                .all(|item| item.repository.source == RepositorySource::Starred)
        );
        let mut reversed_popularity = repos.clone();
        reversed_popularity[0].stars = 0;
        reversed_popularity[1].stars = 10_000_000;
        assert_eq!(
            result
                .matches
                .iter()
                .map(|item| item.relevance_score)
                .collect::<Vec<_>>(),
            discover(&reversed_popularity, request("telegram folder search"), 10)
                .unwrap()
                .matches
                .iter()
                .map(|item| item.relevance_score)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn context_excludes_self_used_and_explicit_repositories_case_insensitively() {
        let repos = vec![
            repo("target", "Telegram search", "Rust", 1),
            repo("used", "Telegram search", "Rust", 1),
            repo("excluded", "Telegram search", "Rust", 1),
            repo("new", "Telegram toolkit", "Go", 1),
            repo("language-only", "File manager written in Rust", "Rust", 1),
        ];
        let mut input = request("owner/target");
        input.context = Some(ProjectContext {
            full_name: "owner/target".into(),
            description: Some("Telegram search written in Rust".into()),
            language: Some("Rust".into()),
            topics: Vec::new(),
            used_repositories: vec!["OWNER/USED".into()],
        });
        input.exclude_full_names.push("OWNER/EXCLUDED".into());
        let result = discover(&repos, input, 10).unwrap();
        assert_eq!(result.excluded_count, 3);
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].repository.full_name, "owner/new");
    }

    #[test]
    fn resolves_cached_target_and_explains_readme_and_topic_matches() {
        let mut target = repo("target", "Telegram toolkit", "Rust", 1);
        target.topics.push("folder-search".into());
        let mut candidate = repo("candidate", "Utility", "Go", 1);
        candidate.topics.push("telegram".into());
        candidate.readme = Some("Find a folder with search".into());
        let result = discover(&[target, candidate], request("OWNER/TARGET"), 10).unwrap();
        assert_eq!(result.matches.len(), 1);
        assert_eq!(
            result.matches[0].repository.why_matched,
            vec![
                "Topics contains: telegram",
                "Cached README contains: folder, search"
            ]
        );
    }

    #[test]
    fn rejects_missing_or_mismatched_context_and_handles_no_matches() {
        let repos = vec![repo("candidate", "Telegram toolkit", "Rust", 1)];
        assert!(discover(&repos, request("owner/missing"), 10).is_err());
        assert!(discover(&repos, request("  "), 10).is_err());
        assert!(discover(&[], request("telegram"), 10).is_err());
        assert!(discover(&repos, request("in the project"), 10).is_err());
        assert!(
            discover(&repos, request("quantum physics"), 10)
                .unwrap()
                .matches
                .is_empty()
        );
        let mut input = request("owner/target");
        input.context = Some(ProjectContext {
            full_name: "owner/other".into(),
            description: None,
            language: None,
            topics: Vec::new(),
            used_repositories: Vec::new(),
        });
        assert!(discover(&repos, input, 10).is_err());
    }

    #[test]
    fn negative_requirement_sentences_do_not_become_project_needs() {
        let repos = vec![
            repo("memory", "Local memory kernel", "Rust", 1),
            repo("llm", "Python LLM database server", "Python", 1),
        ];
        let mut input = request("owner/project");
        input.context = Some(ProjectContext {
            full_name: "owner/project".into(),
            description: Some("Local memory kernel. Operations do not require Python, an LLM account or a database server.".into()),
            language: Some("Rust".into()), topics: Vec::new(), used_repositories: Vec::new(),
        });
        let result = discover(&repos, input, 10).unwrap();
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].repository.full_name, "owner/memory");
        assert!(!result.query_terms.contains(&"llm".to_string()));
        assert!(!result.query_terms.contains(&"python".to_string()));
    }

    #[test]
    fn project_purpose_outweighs_incidental_tools_in_later_sentences() {
        let repos = vec![
            repo("memory", "Memory kernel", "Rust", 1),
            repo("brands", "Claude Codex Grok Antigravity", "Rust", 1),
        ];
        let mut input = request("owner/project");
        input.context = Some(ProjectContext {
            full_name: "owner/project".into(),
            description: Some("Memory kernel. Works with Claude Codex Grok Antigravity.".into()),
            language: Some("Rust".into()),
            topics: Vec::new(),
            used_repositories: Vec::new(),
        });
        let result = discover(&repos, input, 10).unwrap();
        assert_eq!(result.matches[0].repository.full_name, "owner/memory");
        assert_eq!(result.purpose_terms, vec!["kernel", "memory"]);
    }

    #[test]
    fn multiple_purpose_terms_outweigh_one_rare_ambiguous_term() {
        let repos = vec![
            repo("memory", "Local-first memory", "Rust", 1),
            repo("linux", "Linux kernel", "C", 1),
        ];
        let mut input = request("owner/project");
        input.context = Some(ProjectContext {
            full_name: "owner/project".into(),
            description: Some("Local-first memory kernel".into()),
            language: Some("Rust".into()),
            topics: Vec::new(),
            used_repositories: Vec::new(),
        });
        assert_eq!(
            discover(&repos, input, 10).unwrap().matches[0]
                .repository
                .full_name,
            "owner/memory"
        );
    }
}
