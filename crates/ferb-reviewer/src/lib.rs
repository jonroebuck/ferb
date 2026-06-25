use std::path::PathBuf;

use async_trait::async_trait;
use ferb_agent_core::{
    AgentResponse, ArtifactEntry, FerbAgent, HasSwitchboard, KanbanAgent, SwitchboardClient,
    ThreadAgent, Uuid,
};
use ferb_core::{FerbState, KanbanComment, TaskStatus, TramwayClient};
use serde::Deserialize;

fn prompts_dir() -> PathBuf {
    std::env::var("FERB_PROMPTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./prompts"))
}

fn load_prompt(filename: &str) -> anyhow::Result<String> {
    let path = prompts_dir().join(filename);
    std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Failed to load prompt {}: {}", path.display(), e))
}

#[derive(Debug, Deserialize)]
struct ReviewerResponse {
    #[serde(default)]
    pub kanban_update: Option<KanbanUpdate>,
    #[serde(default)]
    pub artifacts: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct KanbanUpdate {
    pub task_id: String,
    pub status: String,
    pub comment: Option<String>,
}

pub struct Reviewer {
    sb: SwitchboardClient,
    tramway: TramwayClient,
}

impl Reviewer {
    pub fn new(switchboard_url: &str, tramway_url: &str, model: &str) -> Self {
        Self {
            sb: SwitchboardClient::new(switchboard_url),
            tramway: TramwayClient::new(tramway_url, model),
        }
    }

    pub async fn run_legacy(
        &self,
        state: &mut FerbState,
        task_id: &str,
    ) -> anyhow::Result<()> {
        let task = match state.kanban_board.get_task(task_id) {
            Some(t) => t.clone(),
            None => return Ok(()),
        };

        if !state.kanban_board.inputs_done(&task) {
            if let Some(t) = state.kanban_board.get_task_mut(task_id) {
                t.status = TaskStatus::Pending;
            }
            return Ok(());
        }

        if task.max_iterations > 0 && task.iterations_used >= task.max_iterations {
            if let Some(t) = state.kanban_board.get_task_mut(task_id) {
                t.status = TaskStatus::Done;
            }
            return Ok(());
        }

        if task.status == TaskStatus::Done {
            return Ok(());
        }

        println!("[ferb-reviewer] Running task: {}", task_id);

        if let Some(t) = state.kanban_board.get_task_mut(task_id) {
            t.status = TaskStatus::InProgress;
            t.iterations_used += 1;
        }

        let prompt_file = task.prompt.as_deref().unwrap_or("reviewer.md");
        let mut system_prompt = load_prompt(prompt_file)?;

        if !task.success_criteria.is_empty() {
            system_prompt.push_str("\n\n## Success Criteria\n");
            for (i, c) in task.success_criteria.iter().enumerate() {
                system_prompt.push_str(&format!("{}. {}\n", i + 1, c));
            }
        }

        let context = build_context(state, task_id);

        let raw = self.tramway.complete(&system_prompt, &context).await?;
        let response: ReviewerResponse = ferb_utils::parse_json(&raw)?;

        if let Some(update) = &response.kanban_update {
            let new_status = match update.status.as_str() {
                "ready_for_review" => TaskStatus::ReadyForReview,
                "done" => TaskStatus::Done,
                "in_progress" => TaskStatus::InProgress,
                _ => TaskStatus::InProgress,
            };

            if let Some(t) = state.kanban_board.get_task_mut(task_id) {
                t.status = new_status;
                if let Some(comment) = &update.comment {
                    t.comments.push(KanbanComment {
                        from: task_id.to_string(),
                        content: comment.clone(),
                        pass: state.pass,
                    });
                }
            }
        }

        if let Some(serde_json::Value::Object(map)) = response.artifacts {
            for (key, value) in map {
                state.set_artifact(&key, value);
            }
        }

        Ok(())
    }
}

impl HasSwitchboard for Reviewer {
    fn switchboard(&self) -> &SwitchboardClient {
        &self.sb
    }
}

#[async_trait]
impl KanbanAgent for Reviewer {}

#[async_trait]
impl ThreadAgent for Reviewer {}

#[async_trait]
impl FerbAgent for Reviewer {
    fn agent_name(&self) -> &str {
        "ferb-reviewer"
    }

    async fn run(
        &self,
        card_id: Uuid,
        state: &FerbState,
    ) -> anyhow::Result<AgentResponse> {
        let task_id = card_id.to_string();

        let task = match state.kanban_board.get_task(&task_id) {
            Some(t) => t.clone(),
            None => return Ok(AgentResponse::noop(&task_id)),
        };

        if !state.kanban_board.inputs_done(&task) || task.status == TaskStatus::Done {
            return Ok(AgentResponse::noop(&task_id));
        }

        if task.max_iterations > 0 && task.iterations_used >= task.max_iterations {
            return Ok(AgentResponse {
                done: true,
                card_id: task_id,
                questions: vec![],
                answers: vec![],
                artifacts: vec![],
                message: "Exhausted iterations".to_string(),
            });
        }

        let prompt_file = task.prompt.as_deref().unwrap_or("reviewer.md");
        let system_prompt = load_prompt(prompt_file)?;
        let context = build_context(state, &task_id);
        let raw = self.tramway.complete(&system_prompt, &context).await?;
        let response: ReviewerResponse = ferb_utils::parse_json(&raw)?;

        let done = response
            .kanban_update
            .as_ref()
            .map(|u| u.status == "done")
            .unwrap_or(false);
        let mut artifacts = vec![];
        if let Some(serde_json::Value::Object(map)) = response.artifacts {
            for (key, value) in map {
                artifacts.push(ArtifactEntry {
                    name: key,
                    content_type: "application/json".to_string(),
                    content: serde_json::to_string(&value).unwrap_or_default(),
                });
            }
        }

        Ok(AgentResponse {
            done,
            card_id: task_id,
            questions: vec![],
            answers: vec![],
            artifacts,
            message: response
                .kanban_update
                .and_then(|u| u.comment)
                .unwrap_or_default(),
        })
    }
}

fn build_context(state: &FerbState, task_id: &str) -> String {
    let task = state.kanban_board.get_task(task_id).unwrap();

    let mut ctx = String::new();

    ctx.push_str(&format!("## Task: {}\n", task.name));
    ctx.push_str(&format!("Status: {:?}\n", task.status));
    ctx.push_str(&format!(
        "Iterations used: {}/{}\n\n",
        task.iterations_used, task.max_iterations
    ));

    ctx.push_str("## Input Artifacts\n");
    for input_id in &task.inputs {
        if let Some(artifact) = state.get_artifact(input_id) {
            ctx.push_str(&format!(
                "### {}\n{}\n\n",
                input_id,
                serde_json::to_string_pretty(artifact).unwrap_or_default()
            ));
        }
    }

    if !task.questions.is_empty() {
        ctx.push_str("## Questions & Answers\n");
        for q in &task.questions {
            ctx.push_str(&format!(
                "Q: {}\nA: {}\n\n",
                q.question,
                q.answer.as_deref().unwrap_or("(unanswered)")
            ));
        }
    }

    if !task.comments.is_empty() {
        ctx.push_str("## Previous Comments\n");
        for c in &task.comments {
            ctx.push_str(&format!("[{}] {}\n", c.from, c.content));
        }
    }

    ctx
}
