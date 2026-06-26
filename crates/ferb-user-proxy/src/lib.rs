use std::io::{self, BufRead, Write};

use async_trait::async_trait;
use ferb_agent_core::{FerbAgent, SwitchboardClient};
use ferb_core::FerbState;

pub struct UserProxy {
    _sb: SwitchboardClient,
}

impl UserProxy {
    pub fn new(switchboard_url: &str) -> Self {
        Self {
            _sb: SwitchboardClient::new(switchboard_url),
        }
    }

    pub fn run_legacy(&self, state: &mut FerbState) -> anyhow::Result<bool> {
        let pending: Vec<_> = state
            .message_channel
            .iter()
            .filter(|m| m.to == "user")
            .cloned()
            .collect();

        if pending.is_empty() {
            return Ok(false);
        }

        let user_replied_tasks: Vec<String> = state
            .message_channel
            .iter()
            .filter(|m| m.from == "user")
            .map(|m| m.task.clone())
            .collect();

        let unanswered: Vec<_> = pending
            .iter()
            .filter(|m| !user_replied_tasks.contains(&m.task))
            .collect();

        if unanswered.is_empty() {
            return Ok(false);
        }

        println!();
        for msg in &unanswered {
            println!("[{}] {}", msg.from, msg.content);
        }

        print!("\nYour response: ");
        io::stdout().flush()?;

        let stdin = io::stdin();
        let mut answer = String::new();
        stdin.lock().read_line(&mut answer)?;
        let answer = answer.trim().to_string();

        if answer.is_empty() {
            return Err(anyhow::anyhow!("Empty response — aborting"));
        }

        let task = unanswered[0].task.clone();
        let agent = unanswered[0].from.clone();
        state.send_message("user", &agent, &task, &answer);

        Ok(true)
    }
}

/// Parse a labeled post's JSON envelope: `{"type": "...", "content": "..."}`.
/// Returns (type, content). Falls back to ("status", raw_content) for non-JSON.
pub fn parse_labeled_post_content(content: &str) -> (String, String) {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
        let t = val["type"].as_str().unwrap_or("status").to_string();
        let c = val["content"].as_str().unwrap_or(content).to_string();
        return (t, c);
    }
    ("status".to_string(), content.to_string())
}

#[async_trait]
impl FerbAgent for UserProxy {
    fn agent_name(&self) -> &str {
        "ferb-user-proxy"
    }

    fn system_prompt(&self) -> &str {
        "You are a user proxy agent that represents the user in the workflow. \
         Read the thread history and post the user's task description to start the thread, \
         or respond to questions on their behalf. \
         Respond with valid JSON only: {\"done\": true/false, \"post\": \"the user's message or task description\"}"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferb_agent_core::{CardContext, Issue, IssueStatus, Post};

    use ferb_core::TramwayClient;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_context() -> CardContext {
        CardContext {
            card: Issue {
                id: "550e8400-e29b-41d4-a716-446655440000".parse().unwrap(),
                title: "Build a blog platform".to_string(),
                status: IssueStatus::Backlog,
            },
            thread_id: "660e8400-e29b-41d4-a716-446655440001".parse().unwrap(),
            channel_id: "770e8400-e29b-41d4-a716-446655440002".parse().unwrap(),
            posts: vec![Post {
                id: "880e8400-e29b-41d4-a716-446655440003".parse().unwrap(),
                author: "ferb-reviewer".to_string(),
                content: "What features does the blog platform need?".to_string(),
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
                "choices": [{"message": {"content": "{\"done\": false, \"post\": \"I need posts, comments, and user auth.\"}"}}]
            })))
            .mount(&tramway)
            .await;

        let agent = UserProxy::new("http://127.0.0.1:1");
        let tc = TramwayClient::new(&tramway.uri(), "test-model");
        let resp = agent.run(make_context(), &tc).await.unwrap();
        assert!(!resp.done);
        assert!(!resp.post.is_empty());
    }

    #[test]
    fn parse_labeled_identifies_question() {
        let raw = r#"{"type":"question","content":"What framework?"}"#;
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "question");
        assert_eq!(c, "What framework?");
    }

    #[test]
    fn parse_labeled_identifies_summary() {
        let raw = r#"{"type":"summary","content":"Refined Goal: Build a todo app."}"#;
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "summary");
        assert!(c.contains("Build a todo app"));
    }

    #[test]
    fn parse_labeled_falls_back_for_plain_text() {
        let raw = "plain text";
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "status");
        assert_eq!(c, "plain text");
    }

    #[test]
    fn parse_labeled_handles_confirmation() {
        let raw = r#"{"type":"confirmation","content":"Goal confirmed."}"#;
        let (t, _) = parse_labeled_post_content(raw);
        assert_eq!(t, "confirmation");
    }

    #[test]
    fn parse_labeled_handles_status() {
        let raw = r#"{"type":"status","content":"Please refine further: needs more detail"}"#;
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "status");
        assert!(c.contains("needs more detail"));
    }

    #[test]
    fn only_question_and_summary_trigger_interaction() {
        let cases = [
            ("question", true),
            ("summary", true),
            ("status", false),
            ("confirmation", false),
        ];
        for (post_type, should_interact) in cases {
            let interacts = post_type == "question" || post_type == "summary";
            assert_eq!(interacts, should_interact, "failed for type={}", post_type);
        }
    }
}
