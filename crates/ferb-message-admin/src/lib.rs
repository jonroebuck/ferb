use async_trait::async_trait;
use ferb_agent_core::{FerbAgent, SwitchboardClient};
use ferb_core::TramwayClient;

pub struct MessageAdmin {
    _sb: SwitchboardClient,
    _tramway: TramwayClient,
}

impl MessageAdmin {
    pub fn new(switchboard_url: &str, tramway_url: &str, model: &str) -> Self {
        Self {
            _sb: SwitchboardClient::new(switchboard_url),
            _tramway: TramwayClient::new(tramway_url, model),
        }
    }
}

#[async_trait]
impl FerbAgent for MessageAdmin {
    fn agent_name(&self) -> &str {
        "ferb-message-admin"
    }

    fn system_prompt(&self) -> &str {
        "You are a message admin agent that monitors thread messages and manages workflow state. \
         Read the thread history and determine if any workflow actions are needed. \
         Action options: none, transition_done, transition_blocked. \
         Respond with valid JSON only: {\"done\": true/false, \"post\": \"your status assessment and action taken\"}"
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
                title: "Monitor workflow state".to_string(),
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
                thread_id: "660e8400-e29b-41d4-a716-446655440001".parse().unwrap(),
                author: "ferb-reviewer".to_string(),
                content: r#"{"done": true, "post": "All tasks complete."}"#.to_string(),
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
                "choices": [{"message": {"content": "{\"done\": true, \"post\": \"All tasks complete. Transitioning to done.\"}"}}]
            })))
            .mount(&tramway)
            .await;

        let agent = MessageAdmin::new("http://127.0.0.1:1", &tramway.uri(), "test-model");
        let tc = TramwayClient::new(&tramway.uri(), "test-model");
        let resp = agent.run(make_context(), &tc).await.unwrap();
        assert!(resp.done);
        assert!(!resp.post.is_empty());
    }
}
