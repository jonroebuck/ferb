use std::path::PathBuf;

use async_trait::async_trait;
use ferb_agent_core::{FerbAgent, SwitchboardClient, Uuid};
use ferb_core::{FerbState, KanbanComment, TaskStatus, TramwayClient};
use serde::Deserialize;

fn prompts_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("FERB_PROMPTS_DIR") {
        return PathBuf::from(dir);
    }
    let local = PathBuf::from("./prompts");
    if local.exists() {
        return local;
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".ferb").join("prompts")
}

fn load_prompt(filename: &str) -> anyhow::Result<String> {
    let path = prompts_dir().join(filename);
    std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Failed to load prompt {}: {}", path.display(), e))
}

#[derive(Debug, Deserialize)]
struct DefineGoalLlmResponse {
    pub done: bool,
    pub post: String,
}

#[derive(Debug, Deserialize, Default)]
struct ReviewerResponse {
    #[serde(default)]
    pub kanban_update: Option<KanbanUpdate>,
    #[serde(default)]
    pub artifact: Option<serde_json::Value>,
    #[serde(default)]
    pub artifact_file: Option<String>,
    #[serde(default)]
    pub artifacts: Option<serde_json::Value>,
}

pub struct Reviewer {
    _sb: SwitchboardClient,
    tramway: TramwayClient,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct KanbanUpdate {
    pub task_id: String,
    pub status: String,
    pub comment: Option<String>,
}

fn extract_content(value: serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s,
        other => serde_json::to_string_pretty(&other).unwrap_or_default(),
    }
}

fn write_single_artifact(
    state: &mut FerbState,
    task_id: &str,
    artifact: serde_json::Value,
    artifact_file: Option<&str>,
) -> anyhow::Result<()> {
    match artifact {
        serde_json::Value::Object(mut obj) => {
            let path = obj
                .remove("artifact_file")
                .or_else(|| obj.remove("file"))
                .or_else(|| obj.remove("path"))
                .and_then(|v| v.as_str().map(|s| s.to_string()));

            let content = obj
                .remove("content")
                .or_else(|| obj.remove("body"))
                .map(extract_content)
                .unwrap_or_else(|| serde_json::to_string_pretty(&serde_json::Value::Object(obj)).unwrap_or_default());

            state.set_artifact(task_id, path.as_deref().or(artifact_file), content)?;
        }
        other => {
            state.set_artifact(task_id, artifact_file, extract_content(other))?;
        }
    }

    Ok(())
}
impl Reviewer {
    pub fn new(switchboard_url: &str, tramway_url: &str, model: &str) -> Self {
        Self {
            _sb: SwitchboardClient::new(switchboard_url),
            tramway: TramwayClient::new(tramway_url, model),
        }
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.tramway = self.tramway.with_max_tokens(n);
        self
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

        let parsed: Option<ReviewerResponse> = ferb_utils::parse_json(&raw).ok();
        let new_status = parsed
            .as_ref()
            .and_then(|r| r.kanban_update.as_ref())
            .map(|u| match u.status.as_str() {
                "ready_for_review" => TaskStatus::ReadyForReview,
                "done" => TaskStatus::Done,
                "in_progress" => TaskStatus::InProgress,
                "failed" => TaskStatus::Failed,
                _ => TaskStatus::Done,
            })
            .unwrap_or(TaskStatus::Done);

        if let Some(t) = state.kanban_board.get_task_mut(task_id) {
            t.status = new_status;
            if let Some(comment) = parsed
                .as_ref()
                .and_then(|r| r.kanban_update.as_ref())
                .and_then(|u| u.comment.clone())
            {
                t.comments.push(KanbanComment {
                    from: task_id.to_string(),
                    content: comment,
                    pass: state.pass,
                });
            } else {
                t.comments.push(KanbanComment {
                    from: task_id.to_string(),
                    content: raw.trim().to_string(),
                    pass: state.pass,
                });
            }
        }

        if let Some(response) = parsed {
            if let Some(artifact) = response.artifact {
                write_single_artifact(state, task_id, artifact, response.artifact_file.as_deref())?;
            }

            if let Some(serde_json::Value::Object(map)) = response.artifacts {
                for (key, value) in map {
                    match value {
                        serde_json::Value::Object(ref obj) => {
                            let content = obj
                                .get("content")
                                .or_else(|| obj.get("body"))
                                .cloned()
                                .map(extract_content)
                                .unwrap_or_else(|| serde_json::to_string_pretty(&value).unwrap_or_default());
                            let file_name = obj
                                .get("artifact_file")
                                .or_else(|| obj.get("file"))
                                .or_else(|| obj.get("path"))
                                .and_then(|v| v.as_str())
                                .unwrap_or(&key);
                            state.set_artifact(&key, Some(file_name), content)?;
                        }
                        _ => {
                            state.set_artifact(&key, Some(&key), extract_content(value))?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Analyze the define-goal thread history and return (done, post).
    /// `done: true` means the reviewer has a refined-goal summary ready to confirm.
    /// `done: false` means the reviewer has a question for the user.
    pub async fn analyze_define_goal_thread(
        &self,
        sb: &SwitchboardClient,
        thread_id: &str,
    ) -> anyhow::Result<(bool, String)> {
        let tid: Uuid = thread_id
            .parse()
            .map_err(|_| anyhow::anyhow!("analyze_define_goal_thread: invalid thread_id={}", thread_id))?;
        let posts = sb.list_posts(tid).await.unwrap_or_default();

        let context = build_thread_context(&posts);
        let system_prompt = load_prompt("define-goal-reviewer.md")?;
        let raw = self.tramway.complete(&system_prompt, &context).await?;

        let resp: DefineGoalLlmResponse = ferb_utils::parse_json(&raw).map_err(|e| {
            anyhow::anyhow!(
                "define-goal reviewer parse error: {}\nRaw (first 300 chars): {}",
                e,
                &raw[..raw.len().min(300)]
            )
        })?;

        Ok((resp.done, resp.post))
    }
}

fn build_thread_context(posts: &[ferb_agent_core::Post]) -> String {
    let mut ctx = String::from("## Thread History\n\n");
    for post in posts {
        let display = extract_inner_content(&post.content);
        ctx.push_str(&format!("[{}]: {}\n\n", post.author, display));
    }
    ctx
}

fn extract_inner_content(content: &str) -> String {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(c) = val["content"].as_str() {
            return c.to_string();
        }
    }
    content.to_string()
}

fn build_context(state: &FerbState, task_id: &str) -> String {
    let task = state.kanban_board.get_task(task_id).unwrap();
    let mut ctx = String::new();

    ctx.push_str(&format!("## Task: {}\n", task.name));
    ctx.push_str(&format!("Status: {:?}\n", task.status));
    ctx.push_str(&format!("Iterations used: {}/{}\n\n", task.iterations_used, task.max_iterations));

    ctx.push_str("## Input Artifacts\n");
    for input_id in &task.inputs {
        if let Some(artifact) = state.get_artifact(input_id) {
            ctx.push_str(&format!("### {}\n{}\n\n", input_id, artifact));
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

#[async_trait]
impl FerbAgent for Reviewer {
    fn agent_name(&self) -> &str {
        "ferb-reviewer"
    }

    fn system_prompt(&self) -> &str {
        "You are a code reviewer agent that reviews work on a software task. \
         Read the thread history and evaluate the current state. \
         Respond with valid JSON only: {\"done\": true/false, \"post\": \"your review comments or approval message\"}"
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
                title: "Add login feature".to_string(),
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
                author: "ferb-worker".to_string(),
                content: "Implemented JWT-based login endpoint.".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
            }],
            input_context: String::new(),
            prompt: None,
        }
    }

    #[tokio::test]
    async fn test_run_returns_valid_agent_response() {
        let tramway = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "{\"done\": false, \"post\": \"Looks good, needs tests.\"}"}}]
            })))
            .mount(&tramway)
            .await;

        let agent = Reviewer::new("http://127.0.0.1:1", &tramway.uri(), "test-model");
        let tc = TramwayClient::new(&tramway.uri(), "test-model");
        let resp = agent.run(make_context(), &tc).await.unwrap();
        assert!(!resp.done);
        assert!(!resp.post.is_empty());
    }

    fn make_post(author: &str, content: &str) -> ferb_agent_core::Post {
        ferb_agent_core::Post {
            id: ferb_agent_core::Uuid::nil(),
            thread_id: ferb_agent_core::Uuid::nil(),
            author: author.to_string(),
            content: content.to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn build_thread_context_formats_posts() {
        let posts = vec![
            make_post("ferb-user-proxy", "Build a todo app"),
            make_post("ferb-reviewer", r#"{"type":"question","content":"What framework?"}"#),
            make_post("ferb-user-proxy", "React"),
        ];
        let ctx = build_thread_context(&posts);
        assert!(ctx.contains("[ferb-user-proxy]: Build a todo app"));
        assert!(ctx.contains("[ferb-reviewer]: What framework?"));
        assert!(ctx.contains("[ferb-user-proxy]: React"));
    }

    #[test]
    fn extract_inner_content_unwraps_json() {
        let content = r#"{"type":"summary","content":"Here is the goal"}"#;
        assert_eq!(extract_inner_content(content), "Here is the goal");
    }

    #[test]
    fn extract_inner_content_falls_back_to_raw() {
        let content = "plain text post";
        assert_eq!(extract_inner_content(content), "plain text post");
    }

    #[test]
    fn build_thread_context_includes_all_authors() {
        let posts = vec![
            make_post("ferb-user-proxy", "goal text"),
            make_post("ferb-reviewer", r#"{"type":"summary","content":"refined"}"#),
        ];
        let ctx = build_thread_context(&posts);
        assert!(ctx.contains("[ferb-user-proxy]"));
        assert!(ctx.contains("[ferb-reviewer]"));
    }
}
