use async_trait::async_trait;
use ferb_agent_core::{
    AgentResponse, FerbAgent, HasSwitchboard, KanbanAgent, SwitchboardClient, ThreadAgent, Uuid,
    Workflow,
};
use ferb_core::{FerbState, KanbanQuestion, QuestionStatus, TaskStatus};

pub struct Moderator {
    sb: SwitchboardClient,
}

impl Moderator {
    pub fn new(switchboard_url: &str) -> Self {
        Self {
            sb: SwitchboardClient::new(switchboard_url),
        }
    }

    pub async fn setup_channels(
        &self,
        workflow: &Workflow,
        state: &mut FerbState,
    ) -> anyhow::Result<()> {
        for ch_def in &workflow.channels {
            let channel = self.sb.create_channel(&ch_def.name).await?;
            state
                .channel_ids
                .insert(ch_def.name.clone(), channel.id.to_string());

            for th_def in &ch_def.threads {
                let thread = self.sb.create_thread(channel.id, &th_def.name).await?;
                let key = format!("{}:{}", ch_def.name, th_def.name);
                state.thread_ids.insert(key, thread.id.to_string());
            }
        }
        Ok(())
    }

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

        let task_ids: Vec<String> =
            state.kanban_board.tasks.iter().map(|t| t.id.clone()).collect();

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

impl HasSwitchboard for Moderator {
    fn switchboard(&self) -> &SwitchboardClient {
        &self.sb
    }
}

#[async_trait]
impl KanbanAgent for Moderator {}

#[async_trait]
impl ThreadAgent for Moderator {}

#[async_trait]
impl FerbAgent for Moderator {
    fn agent_name(&self) -> &str {
        "ferb-moderator"
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
            message: "Channels and threads configured".to_string(),
        })
    }
}
