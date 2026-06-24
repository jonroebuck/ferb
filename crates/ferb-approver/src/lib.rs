use ferb_core::{FerbState, TaskStatus};

pub struct Approver;

impl Approver {
    /// Check if the task this approver is responsible for can be approved.
    /// Conditions:
    /// 1. The target task (approves field) is ReadyForReview
    /// 2. All reviewer tasks that review the target are Done
    /// If both true -> set target to Done, set own task to Done
    pub fn run(&self, state: &mut FerbState, task_id: &str) {
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
