use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct CreateSchema {
    pub resource: String,
    pub required: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IssueResponse {
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
struct UpdateIssueRequest {
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelResponse {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThreadResponse {
    pub id: String,
    pub title: String,
    pub author: String,
}

#[derive(Debug, Serialize)]
struct CreatePostRequest {
    author: String,
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostResponse {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, alias = "timestamp")]
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactResponse {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct SwitchboardRunState {
    pub issue_id: String,
    pub channel_id: String,
    pub thread_id: String,
}

pub struct SwitchboardClient {
    base_url: String,
    http: reqwest::Client,
}

impl SwitchboardClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Verify that Switchboard is reachable by fetching the channels schema.
    /// Fails if the server cannot be contacted or returns a non-2xx response.
    /// Warns (but succeeds) when the response body is not the expected schema format.
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
        if resp.json::<CreateSchema>().await.is_err() {
            eprintln!("[warn] Switchboard returned unexpected schema response — continuing");
        }
        Ok(())
    }

    /// Fetch the create-schema for a resource (e.g. "channels", "issues").
    /// Returns None if the endpoint is unavailable or returns unexpected data.
    async fn get_schema(&self, resource: &str) -> Option<CreateSchema> {
        let url = format!("{}/api/v1/schema/{}", self.base_url, resource);
        let resp = self.http.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json::<CreateSchema>().await.ok()
    }

    /// Build a JSON object with all required fields populated, using an empty
    /// string as the default for any field not supplied by the caller.
    fn apply_schema_defaults(
        base: serde_json::Value,
        schema: &CreateSchema,
    ) -> serde_json::Value {
        let mut map = match base {
            serde_json::Value::Object(m) => m,
            other => return other,
        };
        for field in &schema.required {
            map.entry(field.clone())
                .or_insert_with(|| serde_json::Value::String(String::new()));
        }
        serde_json::Value::Object(map)
    }

    pub async fn create_issue(
        &self,
        title: &str,
        status: &str,
    ) -> anyhow::Result<IssueResponse> {
        let url = format!("{}/api/v1/issues", self.base_url);
        let mut body = serde_json::json!({ "title": title, "status": status });
        if let Some(schema) = self.get_schema("issues").await {
            body = Self::apply_schema_defaults(body, &schema);
        }
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Switchboard create_issue error ({}): {}", status, text);
        }
        let bytes = resp.bytes().await?;
        eprintln!("[trace] create_issue raw response: {}", String::from_utf8_lossy(&bytes));
        serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("create_issue deserialize error: {}", e))
    }

    pub async fn transition_issue(
        &self,
        issue_id: &str,
        status: &str,
    ) -> anyhow::Result<IssueResponse> {
        let url = format!("{}/api/v1/issues/{}/status", self.base_url, issue_id);
        let body = UpdateIssueRequest {
            status: status.to_string(),
        };
        let resp = self.http.patch(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let code = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Switchboard transition_issue error ({}): {}",
                code,
                text
            );
        }
        Ok(resp.json().await?)
    }

    pub async fn create_channel(&self, name: &str) -> anyhow::Result<ChannelResponse> {
        let url = format!("{}/api/v1/channels", self.base_url);
        let mut body = serde_json::json!({ "name": name });
        if let Some(schema) = self.get_schema("channels").await {
            body = Self::apply_schema_defaults(body, &schema);
        }
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Switchboard create_channel error ({}): {}", status, text);
        }
        let bytes = resp.bytes().await?;
        eprintln!("[trace] create_channel raw response: {}", String::from_utf8_lossy(&bytes));
        serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("create_channel deserialize error: {}", e))
    }

    pub async fn create_artifact(&self, name: &str) -> anyhow::Result<ArtifactResponse> {
        let url = format!("{}/api/v1/artifacts", self.base_url);
        let mut body = serde_json::json!({ "name": name });
        if let Some(schema) = self.get_schema("artifacts").await {
            body = Self::apply_schema_defaults(body, &schema);
        }
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Switchboard create_artifact error ({}): {}", status, text);
        }
        Ok(resp.json().await?)
    }

    pub async fn create_thread(
        &self,
        channel_id: &str,
        title: &str,
        author: &str,
    ) -> anyhow::Result<ThreadResponse> {
        let url = format!("{}/api/v1/channels/{}/threads", self.base_url, channel_id);
        println!("[trace] create_thread: POST {} (channel_id={}, title={:?})", url, channel_id, title);
        let mut body = serde_json::json!({ "title": title, "author": author });
        if let Some(schema) = self.get_schema("threads").await {
            body = Self::apply_schema_defaults(body, &schema);
        }
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Switchboard create_thread error ({}): {}", status, text);
        }
        let bytes = resp.bytes().await?;
        eprintln!("[trace] create_thread raw response: {}", String::from_utf8_lossy(&bytes));
        let thread: ThreadResponse = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("create_thread deserialize error: {}", e))?;
        println!("[trace] create_thread: returned thread_id={}", thread.id);
        Ok(thread)
    }

    pub async fn post_to_thread(
        &self,
        thread_id: &str,
        author: &str,
        content: &str,
    ) -> anyhow::Result<PostResponse> {
        let url = format!("{}/api/v1/threads/{}/posts", self.base_url, thread_id);
        println!(
            "[trace] post_to_thread: POST {} (thread_id={}, author={})",
            url, thread_id, author
        );
        let body = CreatePostRequest {
            author: author.to_string(),
            content: content.to_string(),
        };
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Switchboard post_to_thread error ({}): {}", status, text);
        }
        let bytes = resp.bytes().await?;
        eprintln!("[trace] post_to_thread raw response: {}", String::from_utf8_lossy(&bytes));
        let post: PostResponse = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("post_to_thread deserialize error: {}", e))?;
        println!("[trace] post_to_thread: returned post_id={}", post.id);
        Ok(post)
    }

    pub async fn list_thread_posts(
        &self,
        thread_id: &str,
    ) -> anyhow::Result<Vec<PostResponse>> {
        let url = format!("{}/api/v1/threads/{}/posts", self.base_url, thread_id);
        let resp = self.http.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Switchboard list_thread_posts error ({}): {}",
                status,
                text
            );
        }
        Ok(resp.json().await?)
    }
}
