use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct CreateIssueRequest {
    title: String,
    status: String,
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

#[derive(Debug, Serialize)]
struct CreateChannelRequest {
    name: String,
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

    pub async fn create_issue(
        &self,
        title: &str,
        status: &str,
    ) -> anyhow::Result<IssueResponse> {
        let url = format!("{}/api/issues", self.base_url);
        let body = CreateIssueRequest {
            title: title.to_string(),
            status: status.to_string(),
        };
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
        let url = format!("{}/api/issues/{}", self.base_url, issue_id);
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
        let url = format!("{}/api/channels", self.base_url);
        let body = CreateChannelRequest {
            name: name.to_string(),
        };
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
        let url = format!("{}/api/channels/{}/threads", self.base_url, channel_id);
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
            "{}/api/channels/{}/threads/{}/posts",
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
            "{}/api/channels/{}/threads/{}/posts",
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
