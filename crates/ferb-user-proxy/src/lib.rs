use std::io::{self, BufRead, Write};
use ferb_core::FerbState;

pub struct UserProxy;

impl UserProxy {
    /// Check the message channel for any messages directed to "user".
    /// Print them and collect the user's response.
    /// Write the response back to the message channel.
    /// Returns true if user input was collected.
    pub fn run(&self, state: &mut FerbState) -> anyhow::Result<bool> {
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
