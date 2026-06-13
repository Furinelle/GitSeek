use std::path::Path;

use anyhow::{Context, Result};
use tantivy::{
    Index, TantivyDocument,
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::{Field, STORED, Schema, TEXT, Value},
};

use crate::model::RepositoryRecord;

pub struct SearchIndex {
    index: Index,
    full_name: Field,
    owner: Field,
    name: Field,
    description: Field,
    topics: Field,
    language: Field,
    readme: Field,
}

impl SearchIndex {
    pub fn open_or_create(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)
            .with_context(|| format!("failed to create index dir {}", path.display()))?;

        let schema = build_schema();
        let index = Index::open_in_dir(path).or_else(|_| Index::create_in_dir(path, schema))?;
        let schema = index.schema();
        Ok(Self {
            full_name: schema.get_field("full_name")?,
            owner: schema.get_field("owner")?,
            name: schema.get_field("name")?,
            description: schema.get_field("description")?,
            topics: schema.get_field("topics")?,
            language: schema.get_field("language")?,
            readme: schema.get_field("readme")?,
            index,
        })
    }

    pub fn rebuild(&self, repos: &[RepositoryRecord]) -> Result<()> {
        let mut writer = self.index.writer(50_000_000)?;
        writer.delete_all_documents()?;
        for repo in repos {
            writer.add_document(doc!(
                self.full_name => repo.full_name.as_str(),
                self.owner => repo.owner.as_str(),
                self.name => repo.name.as_str(),
                self.description => repo.description.as_deref().unwrap_or_default(),
                self.topics => repo.topics.join(" "),
                self.language => repo.language.as_deref().unwrap_or_default(),
                self.readme => repo.readme.as_deref().unwrap_or_default(),
            ))?;
        }
        writer.commit()?;
        Ok(())
    }

    pub fn search_full_names(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let parser = QueryParser::for_index(
            &self.index,
            vec![
                self.full_name,
                self.owner,
                self.name,
                self.description,
                self.topics,
                self.language,
                self.readme,
            ],
        );
        let query = parser
            .parse_query(query)
            .with_context(|| format!("failed to parse local search query {query:?}"))?;
        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit))?;

        top_docs
            .into_iter()
            .map(|(_, address)| {
                let doc: TantivyDocument = searcher.doc(address)?;
                let full_name = doc
                    .get_first(self.full_name)
                    .and_then(|value| value.as_str())
                    .context("indexed document missing full_name")?;
                Ok(full_name.to_string())
            })
            .collect()
    }
}

fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    builder.add_text_field("full_name", TEXT | STORED);
    builder.add_text_field("owner", TEXT);
    builder.add_text_field("name", TEXT);
    builder.add_text_field("description", TEXT);
    builder.add_text_field("topics", TEXT);
    builder.add_text_field("language", TEXT);
    builder.add_text_field("readme", TEXT);
    builder.build()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::model::RepositorySource;

    #[test]
    fn rebuilds_and_searches_repository_index() {
        let temp = tempfile::tempdir().unwrap();
        let index = SearchIndex::open_or_create(temp.path()).unwrap();
        let repo = RepositoryRecord {
            github_id: 1,
            owner: "modelcontextprotocol".to_string(),
            name: "rust-sdk".to_string(),
            full_name: "modelcontextprotocol/rust-sdk".to_string(),
            url: "https://github.com/modelcontextprotocol/rust-sdk".to_string(),
            description: Some("Rust SDK for MCP".to_string()),
            homepage: None,
            topics: vec!["mcp".to_string(), "rust".to_string()],
            language: Some("Rust".to_string()),
            license: None,
            stars: 10,
            forks: 1,
            watchers: 10,
            created_at: None,
            updated_at: None,
            pushed_at: None,
            starred_at: None,
            last_synced_at: Some(Utc::now()),
            readme_fetched_at: None,
            etag: None,
            source: RepositorySource::Starred,
            readme: Some("Build Model Context Protocol servers".to_string()),
        };

        index.rebuild(&[repo]).unwrap();
        let matches = index.search_full_names("mcp", 5).unwrap();

        assert_eq!(matches, vec!["modelcontextprotocol/rust-sdk"]);
    }
}
