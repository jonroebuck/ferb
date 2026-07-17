use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Shared runtime state for card-based Ferb workflows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FerbState {
    pub artifacts: BTreeMap<String, String>,
    #[serde(default)]
    pub channel_ids: HashMap<String, String>,
    #[serde(default)]
    pub thread_ids: HashMap<String, String>,
    #[serde(default)]
    pub card_ids: HashMap<String, String>,
    #[serde(default)]
    pub agent_assignments: HashMap<String, Vec<String>>,
}

impl Default for FerbState {
    fn default() -> Self {
        Self::new()
    }
}

impl FerbState {
    pub fn new() -> Self {
        Self {
            artifacts: BTreeMap::new(),
            channel_ids: HashMap::new(),
            thread_ids: HashMap::new(),
            card_ids: HashMap::new(),
            agent_assignments: HashMap::new(),
        }
    }

    fn artifacts_dir() -> PathBuf {
        std::env::var("FERB_ARTIFACTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./artifacts"))
    }

    fn validate_artifact_rel_path(path: &str) -> anyhow::Result<&Path> {
        let p = Path::new(path);
        if p.is_absolute() {
            anyhow::bail!("Artifact path must be relative: {}", path);
        }

        if p.components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
        {
            anyhow::bail!("Artifact path cannot escape artifact directory: {}", path);
        }

        Ok(p)
    }

    pub fn set_artifact(
        &mut self,
        task_id: &str,
        file_name: Option<&str>,
        value: impl AsRef<str>,
    ) -> anyhow::Result<()> {
        let dir = Self::artifacts_dir();
        std::fs::create_dir_all(&dir).map_err(|e| {
            anyhow::anyhow!("Failed to create artifacts dir {}: {}", dir.display(), e)
        })?;

        let rel = Self::validate_artifact_rel_path(file_name.unwrap_or(task_id))?;
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to create artifact parent dir {}: {}",
                    parent.display(),
                    e
                )
            })?;
        }

        std::fs::write(&path, value.as_ref())
            .map_err(|e| anyhow::anyhow!("Failed to write artifact {}: {}", path.display(), e))?;

        self.artifacts
            .insert(task_id.to_string(), path.to_string_lossy().into_owned());
        Ok(())
    }

    pub fn get_artifact(&self, task_id: &str) -> Option<String> {
        if let Some(path) = self.artifacts.get(task_id) {
            if let Ok(content) = std::fs::read_to_string(path) {
                return Some(content);
            }
        }

        let dir = Self::artifacts_dir();
        std::fs::read_to_string(dir.join(task_id))
            .ok()
            .or_else(|| std::fs::read_to_string(dir.join(format!("{}.txt", task_id))).ok())
    }
}
