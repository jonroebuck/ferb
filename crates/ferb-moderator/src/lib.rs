use async_trait::async_trait;
use ferb_agent_core::{FerbAgent, SwitchboardClient};
use ferb_core::{FerbState, KanbanQuestion, QuestionStatus, TaskStatus};

pub struct Moderator {
    _sb: SwitchboardClient,
}

impl Moderator {
    pub fn new(switchboard_url: &str) -> Self {
        Self {
            _sb: SwitchboardClient::new(switchboard_url),
        }
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

#[async_trait]
impl FerbAgent for Moderator {
    fn agent_name(&self) -> &str {
        "ferb-moderator"
    }

    fn system_prompt(&self) -> &str {
        "You are a moderator agent that oversees workflow coordination. \
         Read the thread history and ensure the workflow is progressing correctly. \
         Respond with valid JSON only: {\"done\": true/false, \"post\": \"your coordination notes or status update\"}"
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
                title: "Sprint coordination".to_string(),
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
                author: "ferb-worker".to_string(),
                content: "Task A is blocked waiting for API credentials.".to_string(),
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
                "choices": [{"message": {"content": "{\"done\": false, \"post\": \"Escalating API credentials blocker to user.\"}"}}]
            })))
            .mount(&tramway)
            .await;

        let agent = Moderator::new("http://127.0.0.1:1");
        let tc = TramwayClient::new(&tramway.uri(), "test-model");
        let resp = agent.run(make_context(), &tc).await.unwrap();
        assert!(!resp.done);
        assert!(!resp.post.is_empty());
    }
}
