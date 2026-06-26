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

#[derive(Debug, Serialize)]
struct CreateThreadRequest {
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThreadResponse {
    pub id: String,
    pub content: String,
    pub timestamp: String,
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
    pub author: String,
    pub content: String,
    pub timestamp: String,
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

    /// Verify that Switchboard is reachable. Fails if the server cannot be
    /// contacted or returns a non-2xx response. Warns (but succeeds) when the
    /// response body is not the expected schema format.
    pub async fn health_check(&self) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/schema", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Cannot connect to Switchboard at {}", self.base_url))?;
        if !resp.status().is_success() {
            anyhow::bail!("Cannot connect to Switchboard at {}", self.base_url);
        }
        if resp.json::<serde_json::Value>().await.is_err() {
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
        Ok(resp.json().await?)
    }

    pub async fn transition_issue(
        &self,
        issue_id: &str,
        status: &str,
    ) -> anyhow::Result<IssueResponse> {
        let url = format!("{}/api/v1/issues/{}", self.base_url, issue_id);
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
        Ok(resp.json().await?)
    }

    pub async fn create_thread(
        &self,
        channel_id: &str,
        content: &str,
    ) -> anyhow::Result<ThreadResponse> {
        let url = format!("{}/api/v1/channels/{}/threads", self.base_url, channel_id);
        let body = CreateThreadRequest {
            content: content.to_string(),
        };
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Switchboard create_thread error ({}): {}", status, text);
        }
        Ok(resp.json().await?)
    }

    pub async fn post_to_thread(
        &self,
        channel_id: &str,
        thread_id: &str,
        author: &str,
        content: &str,
    ) -> anyhow::Result<PostResponse> {
        let url = format!(
            "{}/api/v1/channels/{}/threads/{}/posts",
            self.base_url, channel_id, thread_id
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
        Ok(resp.json().await?)
    }

    pub async fn list_thread_posts(
        &self,
        channel_id: &str,
        thread_id: &str,
    ) -> anyhow::Result<Vec<PostResponse>> {
        let url = format!(
            "{}/api/v1/channels/{}/threads/{}/posts",
            self.base_url, channel_id, thread_id
        );
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
