use ferb_core::{FerbState, KanbanQuestion, QuestionStatus, TaskStatus};

pub struct Moderator;

impl Moderator {
    /// Reconcile the message channel against the kanban board.
    /// - Agent messages containing questions -> added to task card as Unanswered
    /// - User reply messages -> matched to oldest Unanswered question on that task
    /// - Tasks with Unanswered questions -> kept InProgress
    pub fn reconcile(&self, state: &mut FerbState) {
        let messages = state.message_channel.clone();

        for msg in &messages {
            if msg.from == "user" {
                if let Some(task) = state.kanban_board.get_task_mut(&msg.task) {
                    if let Some(q) = task
                        .questions
                        .iter_mut()
                        .find(|q| q.status == QuestionStatus::Unanswered)
                    {
                        q.answer = Some(msg.content.clone());
                        q.status = QuestionStatus::Answered;
                    }
                }
            } else {
                let content = msg.content.trim();
                let question_lines: Vec<&str> = content
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| l.ends_with('?'))
                    .collect();

                let questions_to_add: Vec<String> = if question_lines.is_empty() {
                    if content.ends_with('?') {
                        vec![content.to_string()]
                    } else {
                        vec![]
                    }
                } else {
                    question_lines.iter().map(|s| s.to_string()).collect()
                };

                for q_text in questions_to_add {
                    if let Some(task) = state.kanban_board.get_task_mut(&msg.task) {
                        let already_exists = task.questions.iter().any(|q| q.question == q_text);
                        if !already_exists {
                            let id = format!("Q{:03}", task.questions.len() + 1);
                            task.questions.push(KanbanQuestion {
                                id,
                                task: msg.task.clone(),
                                question: q_text,
                                answer: None,
                                status: QuestionStatus::Unanswered,
                            });
                        }
                    }
                }
            }
        }

        let task_ids: Vec<String> = state.kanban_board.tasks.iter().map(|t| t.id.clone()).collect();

        for id in task_ids {
            let has_unanswered = state.kanban_board.has_unanswered_questions(&id);
            if let Some(task) = state.kanban_board.get_task_mut(&id) {
                if has_unanswered && task.status != TaskStatus::Done {
                    task.status = TaskStatus::InProgress;
                }
            }
        }
    }
}
