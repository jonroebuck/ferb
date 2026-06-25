use async_trait::async_trait;
use ferb_agent_core::{
    AgentResponse, FerbAgent, HasSwitchboard, KanbanAgent, SwitchboardClient, ThreadAgent, Uuid,
};
use ferb_core::{FerbState, TaskStatus};

pub struct Approver {
    sb: SwitchboardClient,
}

impl Approver {
    pub fn new(switchboard_url: &str) -> Self {
        Self {
            sb: SwitchboardClient::new(switchboard_url),
        }
    }

    pub fn run_legacy(&self, state: &mut FerbState, task_id: &str) {
        let target_id = match state.kanban_board.get_task(task_id) {
            Some(t) => match &t.approves {
                Some(id) => id.clone(),
                None => return,
            },
            None => return,
        };

        let target_ready = state
            .kanban_board
            .get_task(&target_id)
            .map(|t| t.status == TaskStatus::ReadyForReview)
            .unwrap_or(false);

        if !target_ready {
            return;
        }

        let all_reviewers_done = state
            .kanban_board
            .tasks
            .iter()
            .filter(|t| t.reviews.as_deref() == Some(&target_id))
            .all(|t| t.status == TaskStatus::Done);

        if !all_reviewers_done {
            return;
        }

        println!("[ferb-approver] Approving task: {}", target_id);

        if let Some(target) = state.kanban_board.get_task_mut(&target_id) {
            target.status = TaskStatus::Done;
        }
        if let Some(own) = state.kanban_board.get_task_mut(task_id) {
            own.status = TaskStatus::Done;
        }
    }
}

impl HasSwitchboard for Approver {
    fn switchboard(&self) -> &SwitchboardClient {
        &self.sb
    }
}

#[async_trait]
impl KanbanAgent for Approver {}

#[async_trait]
impl ThreadAgent for Approver {}

#[async_trait]
impl FerbAgent for Approver {
    fn agent_name(&self) -> &str {
        "ferb-approver"
    }

    async fn run(
        &self,
        card_id: Uuid,
        state: &FerbState,
    ) -> anyhow::Result<AgentResponse> {
        let task_id = card_id.to_string();

        let target_id = match state.kanban_board.get_task(&task_id) {
            Some(t) => match &t.approves {
                Some(id) => id.clone(),
                None => return Ok(AgentResponse::noop(&task_id)),
            },
            None => return Ok(AgentResponse::noop(&task_id)),
        };

        let target_ready = state
            .kanban_board
            .get_task(&target_id)
            .map(|t| t.status == TaskStatus::ReadyForReview)
            .unwrap_or(false);

        if !target_ready {
            return Ok(AgentResponse::noop(&task_id));
        }

        let all_reviewers_done = state
            .kanban_board
            .tasks
            .iter()
            .filter(|t| t.reviews.as_deref() == Some(&target_id))
            .all(|t| t.status == TaskStatus::Done);

        if !all_reviewers_done {
            return Ok(AgentResponse::noop(&task_id));
        }

        Ok(AgentResponse {
            done: true,
            card_id: task_id,
            questions: vec![],
            answers: vec![],
            artifacts: vec![],
            message: format!("Approved task: {}", target_id),
        })
    }
}
