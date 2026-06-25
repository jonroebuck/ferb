use std::io::{self, BufRead, Write};

use async_trait::async_trait;
use ferb_agent_core::{
    AgentResponse, AnswerRequest, FerbAgent, HasSwitchboard, KanbanAgent, SwitchboardClient,
    ThreadAgent, Uuid,
};
use ferb_core::FerbState;

pub struct UserProxy {
    sb: SwitchboardClient,
}

impl UserProxy {
    pub fn new(switchboard_url: &str) -> Self {
        Self {
            sb: SwitchboardClient::new(switchboard_url),
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

impl HasSwitchboard for UserProxy {
    fn switchboard(&self) -> &SwitchboardClient {
        &self.sb
    }
}

#[async_trait]
impl KanbanAgent for UserProxy {}

#[async_trait]
impl ThreadAgent for UserProxy {}

#[async_trait]
impl FerbAgent for UserProxy {
    fn agent_name(&self) -> &str {
        "ferb-user-proxy"
    }

    async fn run(
        &self,
        card_id: Uuid,
        _state: &FerbState,
    ) -> anyhow::Result<AgentResponse> {
        let questions = self.list_card_questions(card_id).await.unwrap_or_default();

        let unanswered: Vec<_> = questions
            .iter()
            .filter(|q| q.answer.is_none() && q.asked_by != "ferb-user-proxy")
            .collect();

        let mut answers = vec![];
        for q in unanswered {
            println!("[{}] {}", q.asked_by, q.text);
            print!("\nYour response: ");
            io::stdout().flush()?;

            let stdin = io::stdin();
            let mut input = String::new();
            stdin.lock().read_line(&mut input)?;
            let input = input.trim().to_string();

            if !input.is_empty() {
                answers.push(AnswerRequest {
                    question_id: q.id,
                    text: input,
                    answered_by: "ferb-user-proxy".to_string(),
                });
            }
        }

        Ok(AgentResponse {
            done: answers.is_empty() && questions.iter().all(|q| q.answer.is_some()),
            card_id: card_id.to_string(),
            questions: vec![],
            answers,
            artifacts: vec![],
            message: String::new(),
        })
    }
}
