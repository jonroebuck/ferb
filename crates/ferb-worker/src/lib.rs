use std::path::PathBuf;

use async_trait::async_trait;
use ferb_agent_core::{FerbAgent, SwitchboardClient};
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
struct WorkerResponse {
    #[serde(default)]
    pub artifacts: Option<serde_json::Value>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

pub struct Worker {
    _sb: SwitchboardClient,
    tramway: TramwayClient,
}

impl Worker {
    pub fn new(switchboard_url: &str, tramway_url: &str, model: &str) -> Self {
        Self {
            _sb: SwitchboardClient::new(switchboard_url),
            tramway: TramwayClient::new(tramway_url, model),
        }
    }

    pub async fn run_legacy(&self, state: &mut FerbState, task_id: &str) -> anyhow::Result<()> {
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

        let raw = self.tramway.complete(&system_prompt, &context).await?;
        let response: WorkerResponse = ferb_utils::parse_json(&raw)?;

        if let Some(serde_json::Value::Object(map)) = &response.artifacts {
            for (key, value) in map {
                state.set_artifact(key, value.clone());
            }
        }

        let new_status = match response.status.as_deref() {
            Some("ready_for_review") => TaskStatus::ReadyForReview,
            Some("failed") => TaskStatus::Failed,
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

#[async_trait]
impl FerbAgent for Worker {
    fn agent_name(&self) -> &str {
        "ferb-worker"
    }

    fn system_prompt(&self) -> &str {
        "You are a worker agent that implements solutions to software tasks. \
         Read the thread history and implement or continue the current task. \
         You MUST respond with ONLY valid JSON. No prose, no explanation, no markdown. \
         Your entire response must start with { and end with }. \
         Example response: {\"done\": true, \"post\": \"your content here\"}"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferb_agent_core::{CardContext, Issue, IssueStatus, Post, Uuid};

    use ferb_core::TramwayClient;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_context() -> CardContext {
        CardContext {
            card: Issue {
                id: "550e8400-e29b-41d4-a716-446655440000".parse().unwrap(),
                title: "Implement search feature".to_string(),
                status: IssueStatus::InProgress,
                description: String::new(),
                assignee: None,
                created_at: String::new(),
                updated_at: String::new(),
            },
            thread_id: "660e8400-e29b-41d4-a716-446655440001".parse().unwrap(),
            channel_id: "770e8400-e29b-41d4-a716-446655440002".parse().unwrap(),
            posts: vec![Post {
                id: "880e8400-e29b-41d4-a716-446655440003".parse().unwrap(),
                thread_id: Uuid::nil(),
                author: "ferb-user-proxy".to_string(),
                content: "Build a full-text search endpoint using Postgres.".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
            }],
        }
    }

    #[tokio::test]
    async fn test_run_returns_valid_agent_response() {
        let tramway = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "{\"done\": true, \"post\": \"Search endpoint implemented with tsvector indexing.\"}"}}]
            })))
            .mount(&tramway)
            .await;

        let agent = Worker::new("http://127.0.0.1:1", &tramway.uri(), "test-model");
        let tc = TramwayClient::new(&tramway.uri(), "test-model");
        let resp = agent.run(make_context(), &tc).await.unwrap();
        assert!(resp.done);
        assert!(!resp.post.is_empty());
    }
}
