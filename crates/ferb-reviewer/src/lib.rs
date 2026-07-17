use std::path::PathBuf;

use async_trait::async_trait;
use ferb_agent_core::{FerbAgent, SwitchboardClient, Uuid};
use ferb_core::TramwayClient;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct DefineGoalLlmResponse {
    pub done: bool,
    pub post: String,
}

pub struct Reviewer {
    _sb: SwitchboardClient,
    tramway: TramwayClient,
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

    /// Analyze the define-goal thread history and return (done, post).
    /// `done: true` means the reviewer has a refined-goal summary ready to confirm.
    /// `done: false` means the reviewer has a question for the user.
    pub async fn analyze_define_goal_thread(
        &self,
        sb: &SwitchboardClient,
        thread_id: &str,
    ) -> anyhow::Result<(bool, String)> {
        let tid: Uuid = thread_id.parse().map_err(|_| {
            anyhow::anyhow!(
                "analyze_define_goal_thread: invalid thread_id={}",
                thread_id
            )
        })?;
        let posts = sb.list_posts(tid).await.unwrap_or_default();

        let context = build_thread_context(&posts);
        let prompt_path = std::env::var("FERB_PROMPTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./prompts"))
            .join("define-goal-reviewer.md");
        let system_prompt = std::fs::read_to_string(&prompt_path).map_err(|e| {
            anyhow::anyhow!("Failed to load prompt {}: {}", prompt_path.display(), e)
        })?;
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
                title: "Review architecture".to_string(),
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
                content: "Added the new design doc and implementation notes.".to_string(),
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
                "choices": [{"message": {"content": "{\"done\": true, \"post\": \"Looks good overall; approved.\"}"}}]
            })))
            .mount(&tramway)
            .await;

        let agent = Reviewer::new("http://127.0.0.1:1", &tramway.uri(), "test-model");
        let tc = TramwayClient::new(&tramway.uri(), "test-model");
        let resp = agent.run(make_context(), &tc).await.unwrap();
        assert!(resp.done);
        assert!(!resp.post.is_empty());
    }

    #[test]
    fn extract_inner_content_unwraps_json() {
        let raw = r#"{"type":"summary","content":"Refined Goal: Build an app."}"#;
        assert_eq!(extract_inner_content(raw), "Refined Goal: Build an app.");
    }

    #[test]
    fn extract_inner_content_falls_back_to_raw() {
        assert_eq!(extract_inner_content("plain text"), "plain text");
    }

    #[test]
    fn build_thread_context_formats_posts() {
        let posts = vec![Post {
            id: Uuid::nil(),
            thread_id: Uuid::nil(),
            author: "ferb-user-proxy".to_string(),
            content: r#"{"type":"question","content":"What stack?"}"#.to_string(),
            created_at: String::new(),
        }];

        let context = build_thread_context(&posts);
        assert!(context.contains("## Thread History"));
        assert!(context.contains("[ferb-user-proxy]: What stack?"));
    }

    #[test]
    fn build_thread_context_includes_all_authors() {
        let posts = vec![
            Post {
                id: Uuid::nil(),
                thread_id: Uuid::nil(),
                author: "ferb-user-proxy".to_string(),
                content: "Initial goal".to_string(),
                created_at: String::new(),
            },
            Post {
                id: Uuid::nil(),
                thread_id: Uuid::nil(),
                author: "ferb-reviewer".to_string(),
                content: r#"{"type":"summary","content":"Refined summary"}"#.to_string(),
                created_at: String::new(),
            },
        ];

        let context = build_thread_context(&posts);
        assert!(context.contains("[ferb-user-proxy]: Initial goal"));
        assert!(context.contains("[ferb-reviewer]: Refined summary"));
    }
}
