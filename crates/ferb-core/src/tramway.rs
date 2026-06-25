use serde::{Deserialize, Serialize};

pub struct TramwayClient {
    pub base_url: String,
    http: reqwest::Client,
    model: String,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
}

#[derive(Serialize)]
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
        }
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

        let request = ChatRequest {
            model: self.model.clone(),
            messages,
        };

        let response = self
            .http
            .post(&url)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Tramway API error ({}): {}", status, body);
        }

        let chat_response: ChatResponse = response.json().await?;
        let content = chat_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Tramway returned no choices"))?
            .message
            .content;

        Ok(content)
    }
}
