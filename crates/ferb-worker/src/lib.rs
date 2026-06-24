use std::path::PathBuf;
use ferb_core::{FerbState, KanbanComment, TaskStatus};
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
struct WorkerResponse {
    pub artifacts: serde_json::Value,
    pub comment: Option<String>,
    pub status: String,
}

pub struct Worker;

impl Worker {
    pub async fn run(
        &self,
        state: &mut FerbState,
        task_id: &str,
        client: &ferb_core::TramwayClient,
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

        if matches!(task.status, TaskStatus::Done | TaskStatus::ReadyForReview) {
            return Ok(());
        }

        println!("[ferb-worker] Running task: {}", task_id);

        if let Some(t) = state.kanban_board.get_task_mut(task_id) {
            t.status = TaskStatus::InProgress;
        }

        let prompt_file = task.prompt.as_deref().unwrap_or("worker.md");
        let mut system_prompt = load_prompt(prompt_file)?;

        if !task.success_criteria.is_empty() {
            system_prompt.push_str("\n\n## Success Criteria\n");
            for (i, c) in task.success_criteria.iter().enumerate() {
                system_prompt.push_str(&format!("{}. {}\n", i + 1, c));
            }
        }

        let mut context = format!("## Task: {}\n\n", task.name);
        for input_id in &task.inputs {
            if let Some(artifact) = state.get_artifact(input_id) {
                context.push_str(&format!(
                    "### Input: {}\n{}\n\n",
                    input_id,
                    serde_json::to_string_pretty(artifact).unwrap_or_default()
                ));
            }
        }

        let raw = client.complete(&system_prompt, &context).await?;
        let response: WorkerResponse = ferb_utils::parse_json(&raw)?;

        if let serde_json::Value::Object(map) = &response.artifacts {
            for (key, value) in map {
                state.set_artifact(key, value.clone());
            }
        }

        let new_status = match response.status.as_str() {
            "ready_for_review" => TaskStatus::ReadyForReview,
            "failed" => TaskStatus::Failed,
            _ => TaskStatus::ReadyForReview,
        };

        if let Some(t) = state.kanban_board.get_task_mut(task_id) {
            t.status = new_status;
            if let Some(comment) = response.comment {
                t.comments.push(KanbanComment {
                    from: task_id.to_string(),
                    content: comment,
                    pass: state.pass,
                });
            }
        }

        Ok(())
    }
}
