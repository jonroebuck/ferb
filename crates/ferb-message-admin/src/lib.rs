use async_trait::async_trait;
use ferb_agent_core::{FerbAgent, SwitchboardClient, Uuid};
use ferb_core::TramwayClient;

pub struct MessageAdmin {
    sb: SwitchboardClient,
    tramway: TramwayClient,
}

impl MessageAdmin {
    pub fn new(switchboard_url: &str, tramway_url: &str, model: &str) -> Self {
        Self {
            sb: SwitchboardClient::new(switchboard_url),
            tramway: TramwayClient::new(tramway_url, model),
        }
    }

    pub async fn process_thread_messages(
        &self,
        thread_id: Uuid,
        card_id: Uuid,
    ) -> anyhow::Result<String> {
        let posts = self.sb.list_posts(thread_id).await?;
        if posts.is_empty() {
            return Ok("no_action".to_string());
        }

        let latest = &posts[posts.len() - 1];
        let prompt = format!(
            "Interpret this agent message and decide what kanban action to take.\n\
             Possible actions: transition_status, post_comment, no_action\n\
             Message: {}",
            latest.content
        );

        let response = self
            .tramway
            .complete(
                "You are a message admin that interprets agent messages \
                 and decides kanban board actions. Respond with the action name only.",
                &prompt,
            )
            .await?;

        let action = response.trim().to_lowercase();
        if action.contains("transition") || action.contains("done") {
            self.sb.update_issue_status(card_id, "done").await.ok();
        }

        Ok(action)
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
