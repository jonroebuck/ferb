use async_trait::async_trait;
use ferb_agent_core::{FerbAgent, SwitchboardClient};
use ferb_core::TramwayClient;

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

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.tramway = self.tramway.with_max_tokens(n);
        self
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
         Respond with ONLY valid JSON: {\"done\": true, \"post\": \"your content here\"}"
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
