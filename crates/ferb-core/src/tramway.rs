use serde::{Deserialize, Serialize};

pub struct TramwayClient {
    pub base_url: String,
    http: reqwest::Client,
    model: String,
    max_tokens: u32,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
}

#[derive(Serialize, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

impl TramwayClient {
    pub fn new(base_url: &str, model: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            http: reqwest::Client::new(),
            model: model.to_string(),
            max_tokens: 16384,
        }
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    pub async fn complete(&self, system: &str, user: &str) -> anyhow::Result<String> {
        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: system.to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: user.to_string(),
            },
        ];

        self.send_request(messages).await
    }

    pub async fn chat(
        &self,
        system: &str,
        conversation: &[(String, String)],
    ) -> anyhow::Result<String> {
        let mut messages = vec![ChatMessage {
            role: "system".to_string(),
            content: system.to_string(),
        }];

        for (user_msg, assistant_msg) in conversation {
            messages.push(ChatMessage {
                role: "user".to_string(),
                content: user_msg.clone(),
            });
            if !assistant_msg.is_empty() {
                messages.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: assistant_msg.clone(),
                });
            }
        }

        self.send_request(messages).await
    }

    async fn send_request(&self, messages: Vec<ChatMessage>) -> anyhow::Result<String> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let mut all_messages = messages;
        let mut full_content = String::new();
        const MAX_CONTINUATIONS: usize = 3;

        for attempt in 0..=MAX_CONTINUATIONS {
            let request = ChatRequest {
                model: self.model.clone(),
                messages: all_messages.clone(),
                max_tokens: self.max_tokens,
            };

            let response = self.http.post(&url).json(&request).send().await?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!("Tramway API error ({}): {}", status, body);
            }

            let chat_response: ChatResponse = response.json().await?;
            let choice = chat_response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("Tramway returned no choices"))?;

            let finish_reason = choice.finish_reason.as_deref().unwrap_or("unknown");
            let truncated = matches!(finish_reason, "length" | "max_tokens");

            if truncated {
                eprintln!(
                    "[warn] stop_reason={} (max_tokens={}); output may be truncated",
                    finish_reason, self.max_tokens
                );
            } else {
                eprintln!("[trace] stop_reason={}", finish_reason);
            }

            let chunk = choice.message.content;
            full_content.push_str(&chunk);

            if !truncated || attempt == MAX_CONTINUATIONS {
                if truncated {
                    eprintln!(
                        "[warn] Response still truncated after {} continuation(s); using partial output",
                        MAX_CONTINUATIONS
                    );
                }
                break;
            }

            eprintln!(
                "[warn] Requesting continuation (attempt {}/{})...",
                attempt + 1,
                MAX_CONTINUATIONS
            );
            all_messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: chunk,
            });
            all_messages.push(ChatMessage {
                role: "user".to_string(),
                content:
                    "Continue from exactly where you stopped. Output nothing but the continuation."
                        .to_string(),
            });
        }

        Ok(full_content)
    }
}
