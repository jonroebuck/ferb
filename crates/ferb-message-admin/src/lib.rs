use async_trait::async_trait;
use ferb_agent_core::{
    AgentResponse, FerbAgent, HasSwitchboard, IssueStatus, KanbanAgent, SwitchboardClient,
    ThreadAgent, Uuid,
};
use ferb_core::{FerbState, TramwayClient};

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
        let posts = self.sb.list_thread_posts(thread_id).await?;
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
            self.sb
                .update_issue_status(card_id, &IssueStatus::Done)
                .await
                .ok();
        }

        Ok(action)
    }
}

impl HasSwitchboard for MessageAdmin {
    fn switchboard(&self) -> &SwitchboardClient {
        &self.sb
    }
}

#[async_trait]
impl KanbanAgent for MessageAdmin {}

#[async_trait]
impl ThreadAgent for MessageAdmin {}

#[async_trait]
impl FerbAgent for MessageAdmin {
    fn agent_name(&self) -> &str {
        "ferb-message-admin"
    }

    async fn run(
        &self,
        card_id: Uuid,
        _state: &FerbState,
    ) -> anyhow::Result<AgentResponse> {
        let card = self.get_card(card_id).await;
        let all_done = if let Ok(card) = &card {
            card.assigned_agents.iter().all(|_| true)
        } else {
            false
        };

        let done = all_done && card.is_ok();

        let message = if done {
            let card = card.unwrap();
            format!("All agents on card '{}' have completed", card.title)
        } else {
            "Monitoring agent progress".to_string()
        };

        Ok(AgentResponse {
            done,
            card_id: card_id.to_string(),
            questions: vec![],
            answers: vec![],
            artifacts: vec![],
            message,
        })
    }
}
