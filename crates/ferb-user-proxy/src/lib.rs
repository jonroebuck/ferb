use std::io::{self, BufRead, Write};

use async_trait::async_trait;
use ferb_agent_core::{
    AgentResponse, ArtifactEntry, AnswerRequest, FerbAgent, HasSwitchboard, KanbanAgent,
    SwitchboardClient, ThreadAgent, Uuid,
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

/// Parse a labeled post's JSON envelope: `{"type": "...", "content": "..."}`.
/// Returns (type, content). Falls back to ("status", raw_content) for non-JSON.
pub fn parse_labeled_post_content(content: &str) -> (String, String) {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
        let t = val["type"].as_str().unwrap_or("status").to_string();
        let c = val["content"].as_str().unwrap_or(content).to_string();
        return (t, c);
    }
    ("status".to_string(), content.to_string())
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
        state: &FerbState,
    ) -> anyhow::Result<AgentResponse> {
        let card_id_str = card_id.to_string();

        // Thread-based define-goal flow: used when run.rs has set up the Switchboard thread.
        if let (Some(channel_id), Some(thread_id)) = (
            state.channel_ids.get("general"),
            state.thread_ids.get("define-goal"),
        ) {
            return self
                .run_define_goal_turn(&card_id_str, channel_id, thread_id, state)
                .await;
        }

        // Legacy Q&A flow — answer unanswered questions on the card.
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
            card_id: card_id_str,
            questions: vec![],
            answers,
            artifacts: vec![],
            message: String::new(),
        })
    }
}

impl UserProxy {
    /// Handle one turn of the define-goal conversation via the Switchboard thread.
    ///
    /// On first call (no reviewer posts yet): if the goal text is in the message
    /// channel, post it to the thread so the reviewer has something to read.
    ///
    /// On subsequent calls: find the latest reviewer question or summary, display
    /// it, collect the user's stdin response, post it back, and return done=true
    /// only when the user confirms a summary.
    ///
    /// Polls up to 60 s (2 s intervals) waiting for the reviewer to post.
    async fn run_define_goal_turn(
        &self,
        card_id_str: &str,
        channel_id: &str,
        thread_id: &str,
        state: &FerbState,
    ) -> anyhow::Result<AgentResponse> {
        let sb_core = ferb_core::SwitchboardClient::new(&self.sb.base_url);

        // Poll for a reviewer post (up to 60 s, checking every 2 s).
        const POLL_INTERVAL_SECS: u64 = 2;
        const POLL_TIMEOUT_SECS: u64 = 60;
        let max_polls = POLL_TIMEOUT_SECS / POLL_INTERVAL_SECS;

        let mut latest_reviewer_post: Option<ferb_core::PostResponse> = None;

        for poll in 0..=max_polls {
            let posts = sb_core
                .list_thread_posts(channel_id, thread_id)
                .await
                .unwrap_or_default();

            // On the very first poll with no posts, seed the thread with the goal.
            if poll == 0 && posts.is_empty() {
                if let Some(msg) = state
                    .message_channel
                    .iter()
                    .find(|m| m.task == "define-goal" && m.from == "user")
                {
                    let _ = sb_core
                        .post_to_thread(channel_id, thread_id, "ferb-user-proxy", &msg.content)
                        .await;
                }
                return Ok(AgentResponse::noop(card_id_str));
            }

            // Look for the latest post from the reviewer that we should respond to.
            if let Some(post) = posts
                .iter()
                .rev()
                .find(|p| p.author == "ferb-reviewer")
            {
                let (t, _) = parse_labeled_post_content(&post.content);
                if t == "question" || t == "summary" {
                    latest_reviewer_post = Some(post.clone());
                    break;
                }
            }

            if poll < max_polls {
                eprint!("\rWaiting for reviewer... ({}/{}s)", (poll + 1) * POLL_INTERVAL_SECS, POLL_TIMEOUT_SECS);
                let _ = io::stderr().flush();
                tokio::time::sleep(tokio::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;
            } else {
                eprintln!("\n[warn] No reviewer response after {}s — skipping turn.", POLL_TIMEOUT_SECS);
                return Ok(AgentResponse::noop(card_id_str));
            }
        }

        // Clear the "Waiting..." line.
        eprint!("\r                                              \r");
        let _ = io::stderr().flush();

        let post = match latest_reviewer_post {
            Some(p) => p,
            None => return Ok(AgentResponse::noop(card_id_str)),
        };

        let (post_type, display_content) = parse_labeled_post_content(&post.content);

        println!("\n[ferb-reviewer]\n{}\n", display_content);

        if post_type == "summary" {
            print!("Does this look right? (yes/no): ");
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().lock().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            if input == "yes" || input == "y" {
                let _ = sb_core
                    .post_to_thread(
                        channel_id,
                        thread_id,
                        "ferb-user-proxy",
                        &serde_json::json!({"type": "confirmation", "content": "Goal confirmed."})
                            .to_string(),
                    )
                    .await;

                return Ok(AgentResponse {
                    done: true,
                    card_id: card_id_str.to_string(),
                    questions: vec![],
                    answers: vec![],
                    artifacts: vec![ArtifactEntry {
                        name: "define-goal".to_string(),
                        content_type: "text/markdown".to_string(),
                        content: display_content,
                    }],
                    message: "Goal confirmed".to_string(),
                });
            } else {
                print!("What should be different? ");
                io::stdout().flush()?;
                let mut feedback = String::new();
                io::stdin().lock().read_line(&mut feedback)?;
                let feedback = feedback.trim();
                let reply = if feedback.is_empty() {
                    "Please refine further.".to_string()
                } else {
                    format!("Please refine further: {}", feedback)
                };
                let _ = sb_core
                    .post_to_thread(
                        channel_id,
                        thread_id,
                        "ferb-user-proxy",
                        &serde_json::json!({"type": "status", "content": reply}).to_string(),
                    )
                    .await;

                return Ok(AgentResponse::noop(card_id_str));
            }
        }

        // It's a question — collect and post the user's answer.
        print!("Your answer: ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().lock().read_line(&mut answer)?;
        let answer = answer.trim();

        if !answer.is_empty() {
            let _ = sb_core
                .post_to_thread(channel_id, thread_id, "ferb-user-proxy", answer)
                .await
                .map_err(|e| eprintln!("[warn] Failed to post answer: {}", e));
        }

        Ok(AgentResponse::noop(card_id_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_labeled_identifies_question() {
        let raw = r#"{"type":"question","content":"What framework?"}"#;
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "question");
        assert_eq!(c, "What framework?");
    }

    #[test]
    fn parse_labeled_identifies_summary() {
        let raw = r#"{"type":"summary","content":"Refined Goal: Build a todo app."}"#;
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "summary");
        assert!(c.contains("Build a todo app"));
    }

    #[test]
    fn parse_labeled_falls_back_for_plain_text() {
        let raw = "plain text";
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "status");
        assert_eq!(c, "plain text");
    }

    #[test]
    fn parse_labeled_handles_confirmation() {
        let raw = r#"{"type":"confirmation","content":"Goal confirmed."}"#;
        let (t, _) = parse_labeled_post_content(raw);
        assert_eq!(t, "confirmation");
    }

    #[test]
    fn parse_labeled_handles_status() {
        let raw = r#"{"type":"status","content":"Please refine further: needs more detail"}"#;
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "status");
        assert!(c.contains("needs more detail"));
    }

    #[test]
    fn only_question_and_summary_trigger_interaction() {
        let cases = [
            ("question", true),
            ("summary", true),
            ("status", false),
            ("confirmation", false),
        ];
        for (post_type, should_interact) in cases {
            let interacts = post_type == "question" || post_type == "summary";
            assert_eq!(interacts, should_interact, "failed for type={}", post_type);
        }
    }
}
