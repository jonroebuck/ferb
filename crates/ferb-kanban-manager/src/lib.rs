use async_trait::async_trait;
use ferb_agent_core::{
    AgentResponse, FerbAgent, HasSwitchboard, KanbanAgent, SwitchboardClient, ThreadAgent, Uuid,
    Workflow,
};
use ferb_core::FerbState;

pub struct KanbanManager {
    sb: SwitchboardClient,
}

impl KanbanManager {
    pub fn new(switchboard_url: &str) -> Self {
        Self {
            sb: SwitchboardClient::new(switchboard_url),
        }
    }

    pub async fn setup_cards(
        &self,
        workflow: &Workflow,
        state: &mut FerbState,
    ) -> anyhow::Result<()> {
        for card in &workflow.cards {
            let issue = self
                .sb
                .create_issue(&card.title, &card.agents)
                .await?;
            state
                .card_ids
                .insert(card.title.clone(), issue.id.to_string());
            state
                .agent_assignments
                .insert(issue.id.to_string(), card.agents.clone());
        }
        Ok(())
    }
}

impl HasSwitchboard for KanbanManager {
    fn switchboard(&self) -> &SwitchboardClient {
        &self.sb
    }
}

#[async_trait]
impl KanbanAgent for KanbanManager {}

#[async_trait]
impl ThreadAgent for KanbanManager {}

#[async_trait]
impl FerbAgent for KanbanManager {
    fn agent_name(&self) -> &str {
        "ferb-kanban-manager"
    }

    async fn run(
        &self,
        card_id: Uuid,
        _state: &FerbState,
    ) -> anyhow::Result<AgentResponse> {
        Ok(AgentResponse {
            done: true,
            card_id: card_id.to_string(),
            questions: vec![],
            answers: vec![],
            artifacts: vec![],
            message: "Kanban cards created".to_string(),
        })
    }
}
