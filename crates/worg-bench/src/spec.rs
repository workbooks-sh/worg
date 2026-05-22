//! Spec format. A benchmark spec is a small TOML file that names a prompt and
//! a list of validators the model's response must satisfy.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;
use walkdir::WalkDir;

#[derive(Debug, Clone, Deserialize)]
pub struct Spec {
    pub id: String,
    pub category: String,
    pub prompt: String,
    #[serde(default)]
    pub system: Option<String>,
    pub validate: Vec<ValidatorSpec>,
    #[serde(default)]
    pub strip_fences: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValidatorSpec {
    /// worg-parse accepts the output and round-trip is byte-stable
    Parses,
    /// Exactly N headlines in document order
    HeadlineCount { count: usize },
    /// Headline at index has the given TODO/DONE keyword
    StateMatch { headline_index: usize, state: String },
    /// Headline at index has a property in :PROPERTIES: with optional value match
    HasProperty {
        headline_index: usize,
        name: String,
        #[serde(default)]
        value: Option<String>,
    },
    /// Headline at index has a drawer with the given name (e.g. LOGBOOK, NOTES)
    HasDrawer {
        headline_index: usize,
        name: String,
    },
    /// Headline at index includes all listed tags (order-independent)
    TagsContain {
        headline_index: usize,
        tags: Vec<String>,
    },
    /// Headline at index has priority cookie [#X]
    PriorityMatch {
        headline_index: usize,
        priority: String,
    },
    /// Free-form regex over the full response
    Regex { pattern: String },
    /// Output equals the expected string after normalizing whitespace
    EqualsNormalized { expected: String },
    /// Output contains the substring verbatim
    Contains { substring: String },
    /// Headline at index has the given level (1 = `*`, 2 = `**`, …)
    LevelMatch {
        headline_index: usize,
        level: usize,
    },
}

pub fn load_from_path(p: &Path) -> Result<Spec> {
    let text =
        std::fs::read_to_string(p).with_context(|| format!("reading spec {}", p.display()))?;
    let spec: Spec =
        toml::from_str(&text).with_context(|| format!("parsing spec {}", p.display()))?;
    Ok(spec)
}

pub fn load_dir(root: &Path) -> Result<Vec<Spec>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file()
            && entry.path().extension().and_then(|s| s.to_str()) == Some("toml")
        {
            out.push(load_from_path(entry.path())?);
        }
    }
    out.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    Ok(out)
}
