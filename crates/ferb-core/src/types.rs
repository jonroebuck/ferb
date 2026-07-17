use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use serde::{Deserialize, Serialize};

/// A single message on the message channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessage {
    pub from: String,
    pub to: String,
    pub task: String,
    pub content: String,
}

/// A question attached to a kanban task card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanQuestion {
    pub id: String,
    pub task: String,
    pub question: String,
    pub answer: Option<String>,
    pub status: QuestionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStatus {
    Unanswered,
    Answered,
}

/// A comment attached to a kanban task card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanComment {
    pub from: String,
    pub content: String,
    pub pass: usize,
}

/// A single task on the kanban board.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanTask {
    pub id: String,
    pub name: String,
    pub agent: String,
    pub prompt: Option<String>,
    pub status: TaskStatus,
    pub inputs: Vec<String>,
    pub reviews: Option<String>,
    pub approves: Option<String>,
    pub max_iterations: usize,
    pub iterations_used: usize,
    pub questions: Vec<KanbanQuestion>,
    pub comments: Vec<KanbanComment>,
    pub success_criteria: Vec<String>,
    /// Max pipeline passes before the card is marked Blocked. 0 = unlimited.
    #[serde(default = "default_pass_budget")]
    pub pass_budget: usize,
}

fn default_pass_budget() -> usize {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    ReadyForReview,
    Done,
    Blocked,
    Failed,
}

/// The kanban board — shared state across all agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanBoard {
    pub tasks: Vec<KanbanTask>,
}

impl KanbanBoard {
    pub fn get_task(&self, id: &str) -> Option<&KanbanTask> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn get_task_mut(&mut self, id: &str) -> Option<&mut KanbanTask> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    pub fn all_done(&self) -> bool {
        self.tasks.iter().all(|t| t.status == TaskStatus::Done)
    }

    pub fn all_complete(&self) -> bool {
        self.tasks
            .iter()
            .all(|t| t.status == TaskStatus::Done || t.status == TaskStatus::Blocked)
    }

    pub fn inputs_done(&self, task: &KanbanTask) -> bool {
        task.inputs.iter().all(|input_id| {
            self.get_task(input_id)
                .map(|t| matches!(t.status, TaskStatus::Done | TaskStatus::ReadyForReview))
                .unwrap_or(true) // artifact-only inputs (no task with this id) are treated as satisfied
        })
    }

    pub fn has_unanswered_questions(&self, task_id: &str) -> bool {
        self.get_task(task_id)
            .map(|t| {
                t.questions
                    .iter()
                    .any(|q| q.status == QuestionStatus::Unanswered)
            })
            .unwrap_or(false)
    }
}

/// The full shared state passed between every agent on every loop pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FerbState {
    pub message_channel: Vec<ChannelMessage>,
    pub kanban_board: KanbanBoard,
    pub artifacts: BTreeMap<String, String>,
    pub pass: usize,
    #[serde(default)]
    pub active_workflow: Option<serde_json::Value>,
    #[serde(default)]
    pub channel_ids: HashMap<String, String>,
    #[serde(default)]
    pub thread_ids: HashMap<String, String>,
    #[serde(default)]
    pub card_ids: HashMap<String, String>,
    #[serde(default)]
    pub agent_assignments: HashMap<String, Vec<String>>,
}

impl FerbState {
    pub fn new(kanban_board: KanbanBoard) -> Self {
        Self {
            message_channel: vec![],
            kanban_board,
            artifacts: BTreeMap::new(),
            pass: 0,
            active_workflow: None,
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

    pub fn send_message(&mut self, from: &str, to: &str, task: &str, content: &str) {
        self.message_channel.push(ChannelMessage {
            from: from.to_string(),
            to: to.to_string(),
            task: task.to_string(),
            content: content.to_string(),
        });
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
