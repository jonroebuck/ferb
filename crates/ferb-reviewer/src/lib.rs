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
struct ReviewerResponse {
    pub kanban_update: KanbanUpdate,
    pub artifacts: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct KanbanUpdate {
    pub task_id: String,
    pub status: String,
    pub comment: Option<String>,
}

pub struct Reviewer;

impl Reviewer {
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

        if task.max_iterations > 0 && task.iterations_used >= task.max_iterations {
            println!(
                "[ferb-reviewer] {} exhausted iterations — setting done",
                task_id
            );
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

        let raw = client.complete(&system_prompt, &context).await?;
        let response: ReviewerResponse = ferb_utils::parse_json(&raw)?;

        let new_status = match response.kanban_update.status.as_str() {
            "ready_for_review" => TaskStatus::ReadyForReview,
            "done" => TaskStatus::Done,
            "in_progress" => TaskStatus::InProgress,
            _ => TaskStatus::InProgress,
        };

        if let Some(t) = state.kanban_board.get_task_mut(task_id) {
            t.status = new_status;
            if let Some(comment) = response.kanban_update.comment {
                t.comments.push(KanbanComment {
                    from: task_id.to_string(),
                    content: comment,
                    pass: state.pass,
                });
            }
        }

        if let Some(artifacts) = response.artifacts {
            if let serde_json::Value::Object(map) = artifacts {
                for (key, value) in map {
                    state.set_artifact(&key, value);
                }
            }
        }

        Ok(())
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
