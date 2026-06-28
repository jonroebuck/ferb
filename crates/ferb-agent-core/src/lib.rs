pub use uuid::Uuid;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Post {
    pub id: Uuid,
    pub thread_id: Uuid,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    Backlog,
    InProgress,
    Done,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: Uuid,
    pub title: String,
    pub status: IssueStatus,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// Everything an agent needs to process one workflow card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardContext {
    pub card: Issue,
    pub thread_id: Uuid,
    pub channel_id: Uuid,
    /// Full thread history in chronological order.
    pub posts: Vec<Post>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub done: bool,
    /// The text or JSON payload posted back to the thread.
    pub post: String,
}

// ── SwitchboardClient ──────────────────────────────────────────────────────

pub struct SwitchboardClient {
    pub base_url: String,
    http: reqwest::Client,
}

impl SwitchboardClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn health_check(&self) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/schema/channels", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Cannot connect to Switchboard at {}", self.base_url))?;
        if !resp.status().is_success() {
            anyhow::bail!("Cannot connect to Switchboard at {}", self.base_url);
        }
        Ok(())
    }

    pub async fn create_channel(&self, name: &str) -> anyhow::Result<Channel> {
        eprintln!("[info] Switchboard: creating channel \"{}\"", name);
        let url = format!("{}/api/v1/channels", self.base_url);
        let body = serde_json::json!({ "name": name, "description": "" });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("create_channel error ({}): {}", code, text);
        }
        let bytes = resp.bytes().await?;
        eprintln!("[trace] create_channel raw response: {}", String::from_utf8_lossy(&bytes));
        let ch: Channel = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("create_channel deserialize error: {}", e))?;
        eprintln!("[info] Switchboard: channel created id={}", ch.id);
        Ok(ch)
    }

    pub async fn create_thread(&self, channel_id: Uuid, title: &str) -> anyhow::Result<Thread> {
        eprintln!(
            "[info] Switchboard: creating thread \"{}\" in channel {}",
            title, channel_id
        );
        let url = format!("{}/api/v1/channels/{}/threads", self.base_url, channel_id);
        let body = serde_json::json!({ "title": title, "author": "ferb" });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("create_thread error ({}): {}", code, text);
        }
        let bytes = resp.bytes().await?;
        eprintln!("[trace] create_thread raw response: {}", String::from_utf8_lossy(&bytes));
        let th: Thread = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("create_thread deserialize error: {}", e))?;
        eprintln!("[info] Switchboard: thread created id={}", th.id);
        Ok(th)
    }

    pub async fn list_posts(&self, thread_id: Uuid) -> anyhow::Result<Vec<Post>> {
        let url = format!("{}/api/v1/threads/{}/posts", self.base_url, thread_id);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("list_posts error ({}): {}", code, text);
        }
        Ok(resp.json().await?)
    }

    pub async fn post_to_thread(
        &self,
        thread_id: Uuid,
        author: &str,
        content: &str,
    ) -> anyhow::Result<Post> {
        eprintln!(
            "[info] Switchboard: posting to thread {} as {}: \"{}\"",
            thread_id, author, content
        );
        let url = format!("{}/api/v1/threads/{}/posts", self.base_url, thread_id);
        let body = serde_json::json!({ "author": author, "content": content });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("post_to_thread error ({}): {}", code, text);
        }
        let bytes = resp.bytes().await?;
        eprintln!("[trace] post_to_thread raw response: {}", String::from_utf8_lossy(&bytes));
        let post: Post = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("post_to_thread deserialize error: {}", e))?;
        eprintln!("[info] Switchboard: post created id={}", post.id);
        Ok(post)
    }

    pub async fn create_issue(&self, title: &str) -> anyhow::Result<Issue> {
        eprintln!("[info] Switchboard: creating issue \"{}\"", title);
        let url = format!("{}/api/v1/issues", self.base_url);
        let body = serde_json::json!({ "title": title, "description": "" });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("create_issue error ({}): {}", code, text);
        }
        let bytes = resp.bytes().await?;
        eprintln!("[trace] create_issue raw response: {}", String::from_utf8_lossy(&bytes));
        let issue: Issue = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("create_issue deserialize error: {}", e))?;
        eprintln!("[info] Switchboard: issue created id={}", issue.id);
        Ok(issue)
    }

    /// PATCH /api/v1/issues/{id}/status
    pub async fn update_issue_status(&self, id: Uuid, status: &str) -> anyhow::Result<()> {
        eprintln!("[info] Switchboard: updating issue {} status to {}", id, status);
        let url = format!("{}/api/v1/issues/{}/status", self.base_url, id);
        let body = serde_json::json!({ "status": status });
        let resp = self.http.patch(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("update_issue_status error ({}): {}", code, text);
        }
        Ok(())
    }

    pub async fn get_issue(&self, id: Uuid) -> anyhow::Result<Issue> {
        let url = format!("{}/api/v1/issues/{}", self.base_url, id);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("get_issue error ({}): {}", code, text);
        }
        Ok(resp.json().await?)
    }
}

// ── FerbAgent Trait ────────────────────────────────────────────────────────

pub fn format_thread_history(posts: &[Post]) -> String {
    let mut ctx = String::from("## Thread History\n\n");
    for post in posts {
        ctx.push_str(&format!("[{}]: {}\n\n", post.author, post.content));
    }
    ctx
}

pub fn parse_agent_response(raw: &str) -> anyhow::Result<AgentResponse> {
    let s = raw.trim();
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .map(|t| t.trim_start_matches('\n').trim_end_matches("```").trim())
        .unwrap_or(s);
    serde_json::from_str(s)
        .map_err(|e| anyhow::anyhow!("Failed to parse AgentResponse: {}\nRaw: {}", e, raw))
}

#[async_trait]
pub trait FerbAgent: Send + Sync {
    fn agent_name(&self) -> &str;
    fn system_prompt(&self) -> &str;

    /// Default implementation: format thread history, call Tramway, parse
    /// the LLM response as AgentResponse JSON.
    async fn run(
        &self,
        context: CardContext,
        tramway: &ferb_core::TramwayClient,
    ) -> anyhow::Result<AgentResponse> {
        let history = format_thread_history(&context.posts);
        let raw = tramway.complete(self.system_prompt(), &history).await?;
        parse_agent_response(&raw)
    }
}
