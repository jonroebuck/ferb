pub use uuid::Uuid;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ── Types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: Uuid,
    pub title: String,
    pub status: IssueStatus,
    #[serde(default)]
    pub assigned_agents: Vec<String>,
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
pub struct IssueEvent {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub agent: String,
    pub content: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub id: Uuid,
    pub text: String,
    pub asked_by: String,
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub answered_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub threads: Vec<Thread>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Post {
    pub id: Uuid,
    pub thread_id: Uuid,
    pub content: String,
    pub author: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub done: bool,
    pub card_id: String,
    #[serde(default)]
    pub questions: Vec<QuestionRequest>,
    #[serde(default)]
    pub answers: Vec<AnswerRequest>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactEntry>,
    #[serde(default)]
    pub message: String,
}

impl AgentResponse {
    pub fn noop(card_id: &str) -> Self {
        Self {
            done: false,
            card_id: card_id.to_string(),
            questions: vec![],
            answers: vec![],
            artifacts: vec![],
            message: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionRequest {
    pub id: Uuid,
    pub text: String,
    pub asked_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerRequest {
    pub question_id: Uuid,
    pub text: String,
    pub answered_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactEntry {
    pub name: String,
    pub content_type: String,
    pub content: String,
}

// ── SwitchboardClient ──

pub struct SwitchboardClient {
    pub base_url: String,
    http: reqwest::Client,
}

impl SwitchboardClient {
    pub fn new(switchboard_url: &str) -> Self {
        Self {
            base_url: switchboard_url.to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn get_issue(&self, card_id: Uuid) -> anyhow::Result<Issue> {
        let url = format!("{}/api/v1/issues/{}", self.base_url, card_id);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("GET {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn update_issue_status(
        &self,
        card_id: Uuid,
        status: &IssueStatus,
    ) -> anyhow::Result<IssueEvent> {
        let url = format!("{}/api/v1/issues/{}", self.base_url, card_id);
        let body = serde_json::json!({ "status": status });
        let resp = self.http.patch(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("PATCH {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn post_issue_comment(
        &self,
        card_id: Uuid,
        agent: &str,
        comment: &str,
    ) -> anyhow::Result<IssueEvent> {
        let url = format!("{}/api/v1/issues/{}/comments", self.base_url, card_id);
        let body = serde_json::json!({ "agent": agent, "content": comment });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("POST {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn list_questions(&self, card_id: Uuid) -> anyhow::Result<Vec<Question>> {
        let url = format!("{}/api/v1/issues/{}/questions", self.base_url, card_id);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("GET {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn post_question(
        &self,
        card_id: Uuid,
        text: &str,
        asked_by: &str,
    ) -> anyhow::Result<Question> {
        let url = format!("{}/api/v1/issues/{}/questions", self.base_url, card_id);
        let body = serde_json::json!({ "text": text, "asked_by": asked_by });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("POST {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn answer_question(
        &self,
        card_id: Uuid,
        question_id: Uuid,
        answer: &str,
        answered_by: &str,
    ) -> anyhow::Result<()> {
        let url = format!(
            "{}/api/v1/issues/{}/questions/{}/answers",
            self.base_url, card_id, question_id
        );
        let body = serde_json::json!({ "text": answer, "answered_by": answered_by });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("POST {} failed: {}", url, resp.status());
        }
        Ok(())
    }

    pub async fn get_channel(&self, channel_id: Uuid) -> anyhow::Result<Channel> {
        let url = format!("{}/api/v1/channels/{}", self.base_url, channel_id);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("GET {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn create_channel(&self, name: &str) -> anyhow::Result<Channel> {
        let url = format!("{}/api/v1/channels", self.base_url);
        let body = serde_json::json!({ "name": name });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("POST {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn create_thread(
        &self,
        channel_id: Uuid,
        title: &str,
    ) -> anyhow::Result<Thread> {
        let url = format!("{}/api/v1/channels/{}/threads", self.base_url, channel_id);
        let body = serde_json::json!({ "title": title });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("POST {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn post_to_thread(
        &self,
        thread_id: Uuid,
        author: &str,
        content: &str,
    ) -> anyhow::Result<Post> {
        let url = format!("{}/api/v1/threads/{}/posts", self.base_url, thread_id);
        let body = serde_json::json!({ "author": author, "content": content });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("POST {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn list_thread_posts(&self, thread_id: Uuid) -> anyhow::Result<Vec<Post>> {
        let url = format!("{}/api/v1/threads/{}/posts", self.base_url, thread_id);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("GET {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }

    pub async fn create_issue(
        &self,
        title: &str,
        agents: &[String],
    ) -> anyhow::Result<Issue> {
        let url = format!("{}/api/v1/issues", self.base_url);
        let body = serde_json::json!({
            "title": title,
            "status": "backlog",
            "assigned_agents": agents,
        });
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("POST {} failed: {}", url, resp.status());
        }
        Ok(resp.json().await?)
    }
}

// ── Traits ──

pub trait HasSwitchboard {
    fn switchboard(&self) -> &SwitchboardClient;
}

#[async_trait]
pub trait KanbanAgent: HasSwitchboard + Send + Sync {
    async fn get_card(&self, card_id: Uuid) -> anyhow::Result<Issue> {
        self.switchboard().get_issue(card_id).await
    }

    async fn update_card_status(
        &self,
        card_id: Uuid,
        status: IssueStatus,
    ) -> anyhow::Result<IssueEvent> {
        self.switchboard().update_issue_status(card_id, &status).await
    }

    async fn post_card_comment(
        &self,
        card_id: Uuid,
        comment: &str,
    ) -> anyhow::Result<IssueEvent> {
        self.switchboard()
            .post_issue_comment(card_id, "system", comment)
            .await
    }

    async fn list_card_questions(&self, card_id: Uuid) -> anyhow::Result<Vec<Question>> {
        self.switchboard().list_questions(card_id).await
    }

    async fn answer_question(
        &self,
        card_id: Uuid,
        question_id: Uuid,
        answer: &str,
    ) -> anyhow::Result<()> {
        self.switchboard()
            .answer_question(card_id, question_id, answer, "system")
            .await
    }

    async fn post_question(
        &self,
        card_id: Uuid,
        question: &str,
    ) -> anyhow::Result<Question> {
        self.switchboard()
            .post_question(card_id, question, "system")
            .await
    }
}

#[async_trait]
pub trait ThreadAgent: HasSwitchboard + Send + Sync {
    async fn get_channel(&self, channel_id: Uuid) -> anyhow::Result<Channel> {
        self.switchboard().get_channel(channel_id).await
    }

    async fn post_to_thread(
        &self,
        thread_id: Uuid,
        content: &str,
    ) -> anyhow::Result<Post> {
        self.switchboard()
            .post_to_thread(thread_id, "system", content)
            .await
    }

    async fn list_thread_posts(&self, thread_id: Uuid) -> anyhow::Result<Vec<Post>> {
        self.switchboard().list_thread_posts(thread_id).await
    }

    async fn create_thread(
        &self,
        channel_id: Uuid,
        title: &str,
    ) -> anyhow::Result<Thread> {
        self.switchboard().create_thread(channel_id, title).await
    }
}

#[async_trait]
pub trait FerbAgent: KanbanAgent + ThreadAgent {
    fn agent_name(&self) -> &str;

    async fn run(
        &self,
        card_id: Uuid,
        state: &ferb_core::FerbState,
    ) -> anyhow::Result<AgentResponse>;
}

// ── Workflow ──

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Workflow {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub channels: Vec<WorkflowChannel>,
    #[serde(default)]
    pub cards: Vec<WorkflowCard>,
    #[serde(default)]
    pub agents: Vec<WorkflowAgentDef>,
    #[serde(default)]
    pub steps: Vec<WorkflowStep>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowChannel {
    pub name: String,
    #[serde(default)]
    pub threads: Vec<WorkflowThread>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowThread {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowCard {
    pub title: String,
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowAgentDef {
    pub name: String,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowStep {
    pub name: String,
    #[serde(default)]
    pub agent: Option<String>,
    pub task: String,
    #[serde(default)]
    pub depends_on: Option<DependsOn>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DependsOn {
    Single(String),
    Multiple(Vec<String>),
}

pub fn parse_workflow(yaml: &str) -> anyhow::Result<Workflow> {
    let workflow: Workflow = serde_yaml::from_str(yaml)?;
    Ok(workflow)
}

impl Workflow {
    pub fn is_bootstrap(&self) -> bool {
        !self.steps.is_empty() && self.cards.is_empty()
    }
}
