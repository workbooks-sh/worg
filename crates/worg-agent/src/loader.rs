//! Read a `.org` file and extract one or more [`AgentSpec`]s.
//!
//! Thin wrapper around [`worg_orch::agent_definition_by_id`] /
//! [`worg_orch::agent_definitions`] — the heavy lifting (parsing,
//! property extraction, system-prompt subtree collection) lives in
//! the orch walker. This module just adapts `AgentDefinition` → our
//! [`AgentSpec`] runtime shape.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use thiserror::Error;

use crate::types::AgentSpec;

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("failed to read agent file {path:?}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no `:agent:`-tagged headline found in {path:?}")]
    NoAgentHeadline { path: std::path::PathBuf },
    #[error("no agent with :ID: {wanted:?} found in {path:?} (available: {found:?})")]
    AgentNotFound {
        path: std::path::PathBuf,
        wanted: String,
        found: Vec<String>,
    },
    #[error("agent {id:?} in {path:?} has no :MODEL: property")]
    MissingModel {
        path: std::path::PathBuf,
        id: String,
    },
}

/// Load every agent declared in `path`. Returns one [`AgentSpec`] per
/// `:agent:`-tagged level-1 headline.
pub fn load_all(path: impl AsRef<Path>) -> Result<Vec<AgentSpec>, LoadError> {
    let path = path.as_ref();
    let src = fs::read_to_string(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    let doc = worg_parse::Document::parse(&src);
    let defs = worg_orch::agent_definitions(&doc);

    if defs.is_empty() {
        return Err(LoadError::NoAgentHeadline {
            path: path.to_path_buf(),
        });
    }

    defs.into_iter()
        .map(|def| to_spec(def, path))
        .collect::<Result<Vec<_>, _>>()
}

/// Load one agent by its `:ID:` from `path`. Falls back to the first
/// agent in the file when `id` is `None` — convenient for single-agent
/// files like `agents/wavelet-director.org`.
pub fn load_one(path: impl AsRef<Path>, id: Option<&str>) -> Result<AgentSpec, LoadError> {
    let path = path.as_ref();
    let mut all = load_all(path)?;
    match id {
        Some(id) => all
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| LoadError::AgentNotFound {
                path: path.to_path_buf(),
                wanted: id.into(),
                found: load_all(path)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| a.id)
                    .collect(),
            }),
        None => Ok(all.remove(0)),
    }
}

fn to_spec(def: worg_orch::AgentDefinition, path: &Path) -> Result<AgentSpec, LoadError> {
    let id = def.wire.id.as_str().to_string();
    let model = def.model.ok_or_else(|| LoadError::MissingModel {
        path: path.to_path_buf(),
        id: id.clone(),
    })?;

    Ok(AgentSpec {
        id,
        title: def.wire.name,
        model,
        system_prompt: def.system_prompt,
        capabilities: def.wire.capabilities,
        tools: def.tools,
        // worg-orch's walker doesn't surface every property — extras
        // can be added in a follow-up if a use case appears.
        extra_properties: BTreeMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_org(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new()
            .suffix(".org")
            .tempfile()
            .unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f
    }

    #[test]
    fn loads_a_minimal_single_agent_file() {
        let f = write_org(
            "* My Agent                                                  :agent:\n\
             :PROPERTIES:\n\
             :ID:           my-agent\n\
             :MODEL:        openrouter/test-model\n\
             :CAPABILITIES: bash read\n\
             :TOOLS:        bash read write\n\
             :END:\n",
        );
        let spec = load_one(f.path(), None).unwrap();
        assert_eq!(spec.id, "my-agent");
        assert_eq!(spec.title, "My Agent");
        assert_eq!(spec.model, "openrouter/test-model");
        assert_eq!(spec.capabilities, vec!["bash", "read"]);
        assert_eq!(spec.tools, vec!["bash", "read", "write"]);
        assert!(spec.system_prompt.is_none());
    }

    #[test]
    fn extracts_system_prompt_subtree() {
        let f = write_org(
            "* My Agent                                                  :agent:\n\
             :PROPERTIES:\n\
             :ID:    my-agent\n\
             :MODEL: openrouter/test-model\n\
             :END:\n\
             ** System prompt\n\
             You are a helpful assistant.\n",
        );
        let spec = load_one(f.path(), None).unwrap();
        assert!(spec.system_prompt.is_some());
        assert!(spec
            .system_prompt
            .as_deref()
            .unwrap()
            .contains("You are a helpful assistant"));
    }

    #[test]
    fn errors_when_no_agent_tag() {
        let f = write_org("* Just a Headline\nNo agent tag here.\n");
        match load_one(f.path(), None).unwrap_err() {
            LoadError::NoAgentHeadline { .. } => {}
            other => panic!("expected NoAgentHeadline, got {other:?}"),
        }
    }

    #[test]
    fn errors_when_model_missing() {
        let f = write_org(
            "* Bare Agent                                                :agent:\n\
             :PROPERTIES:\n\
             :ID: bare\n\
             :END:\n",
        );
        match load_one(f.path(), None).unwrap_err() {
            LoadError::MissingModel { id, .. } => assert_eq!(id, "bare"),
            other => panic!("expected MissingModel, got {other:?}"),
        }
    }
}
