use ferb_agent_core::*;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_uuid() -> Uuid {
    "550e8400-e29b-41d4-a716-446655440000".parse().unwrap()
}

fn channel_json(id: Uuid, name: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "name": name })
}

fn thread_json(id: Uuid, channel_id: Uuid, title: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "channel_id": channel_id, "title": title })
}

fn issue_json(id: Uuid, title: &str, status: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "title": title, "status": status })
}

fn post_json(id: Uuid, thread_id: Uuid, author: &str, content: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "thread_id": thread_id,
        "author": author,
        "content": content,
        "created_at": "2026-01-01T00:00:00Z"
    })
}

// ── health_check ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_health_check_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/schema/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    assert!(client.health_check().await.is_ok());
}

#[tokio::test]
async fn test_health_check_fails_when_unreachable() {
    let client = SwitchboardClient::new("http://127.0.0.1:1");
    let err = client.health_check().await.unwrap_err();
    assert!(err.to_string().contains("Cannot connect to Switchboard"));
}

#[tokio::test]
async fn test_health_check_fails_on_non_2xx() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/schema/channels"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let err = client.health_check().await.unwrap_err();
    assert!(err.to_string().contains("Cannot connect to Switchboard"));
}

// ── create_channel ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_channel() {
    let server = MockServer::start().await;
    let id = test_uuid();
    Mock::given(method("POST"))
        .and(path("/api/v1/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(channel_json(id, "general")))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let ch = client.create_channel("general").await.unwrap();
    assert_eq!(ch.id, id);
    assert_eq!(ch.name, "general");
}

// ── create_thread ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_thread_sends_author_ferb() {
    let server = MockServer::start().await;
    let ch_id = test_uuid();
    let th_id: Uuid = "660e8400-e29b-41d4-a716-446655440001".parse().unwrap();

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/channels/{}/threads", ch_id)))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "title": "progress",
            "author": "ferb"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(thread_json(th_id, ch_id, "progress")))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let th = client.create_thread(ch_id, "progress").await.unwrap();
    assert_eq!(th.id, th_id);
    assert_eq!(th.channel_id, ch_id);
    assert_eq!(th.title, "progress");
}

// ── list_posts ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_list_posts() {
    let server = MockServer::start().await;
    let th_id = test_uuid();
    let post_id: Uuid = "770e8400-e29b-41d4-a716-446655440002".parse().unwrap();

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/threads/{}/posts", th_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            post_json(post_id, th_id, "ferb-user-proxy", "Build a todo app")
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let posts = client.list_posts(th_id).await.unwrap();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].author, "ferb-user-proxy");
    assert_eq!(posts[0].content, "Build a todo app");
}

// ── post_to_thread ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_post_to_thread() {
    let server = MockServer::start().await;
    let th_id = test_uuid();
    let post_id: Uuid = "880e8400-e29b-41d4-a716-446655440003".parse().unwrap();

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/threads/{}/posts", th_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            post_json(post_id, th_id, "ferb-reviewer", r#"{"done":false,"post":"looks good"}"#),
        ))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let post = client
        .post_to_thread(th_id, "ferb-reviewer", r#"{"done":false,"post":"looks good"}"#)
        .await
        .unwrap();
    assert_eq!(post.author, "ferb-reviewer");
}

// ── create_issue ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_issue() {
    let server = MockServer::start().await;
    let id = test_uuid();
    Mock::given(method("POST"))
        .and(path("/api/v1/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(id, "My task", "backlog")))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let issue = client.create_issue("My task").await.unwrap();
    assert_eq!(issue.id, id);
    assert_eq!(issue.title, "My task");
    assert_eq!(issue.status, IssueStatus::Backlog);
}

// ── update_issue_status ───────────────────────────────────────────────────

#[tokio::test]
async fn test_update_issue_status() {
    let server = MockServer::start().await;
    let id = test_uuid();
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/issues/{}/status", id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": id, "status": "done"})))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    assert!(client.update_issue_status(id, "done").await.is_ok());
}

// ── get_issue ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_get_issue() {
    let server = MockServer::start().await;
    let id = test_uuid();
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/issues/{}", id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(id, "My task", "in_progress")))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let issue = client.get_issue(id).await.unwrap();
    assert_eq!(issue.status, IssueStatus::InProgress);
}

// ── test_channel_thread_post_flow ─────────────────────────────────────────

#[tokio::test]
async fn test_channel_thread_post_flow() {
    let server = MockServer::start().await;

    let ch_id: Uuid = "110e8400-e29b-41d4-a716-446655440000".parse().unwrap();
    let th_id: Uuid = "220e8400-e29b-41d4-a716-446655440001".parse().unwrap();
    let post_id: Uuid = "330e8400-e29b-41d4-a716-446655440002".parse().unwrap();

    Mock::given(method("POST"))
        .and(path("/api/v1/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(channel_json(ch_id, "ferb-general")))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/channels/{}/threads", ch_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(thread_json(th_id, ch_id, "Define Goal: test task")))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/threads/{}/posts", th_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(post_json(post_id, th_id, "ferb-user-proxy", "Build a todo app")))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());

    // Step 1 & 2: create channel, assert id returned
    let ch = client.create_channel("ferb-general").await.unwrap_or_else(|e| {
        panic!(
            "create_channel failed\nSent: POST /api/v1/channels body={{\"name\":\"ferb-general\"}}\nError: {}",
            e
        )
    });
    assert_eq!(
        ch.id, ch_id,
        "create_channel: received id={} but expected id={}",
        ch.id, ch_id
    );

    // Step 3 & 4: create thread, assert id returned
    let th = client
        .create_thread(ch.id, "Define Goal: test task")
        .await
        .unwrap_or_else(|e| {
            panic!(
                "create_thread failed\nSent: POST /api/v1/channels/{}/threads body={{\"title\":\"Define Goal: test task\"}}\nError: {}",
                ch.id, e
            )
        });
    assert_eq!(
        th.id, th_id,
        "create_thread: received id={} but expected id={}",
        th.id, th_id
    );

    // Step 5 & 6: post to thread, assert post id returned
    let post = client
        .post_to_thread(th.id, "ferb-user-proxy", "Build a todo app")
        .await
        .unwrap_or_else(|e| {
            panic!(
                "post_to_thread failed\nSent: POST /api/v1/threads/{}/posts body={{\"author\":\"ferb-user-proxy\",\"content\":\"Build a todo app\"}}\nError: {}",
                th.id, e
            )
        });
    assert_eq!(
        post.id, post_id,
        "post_to_thread: received id={} but expected id={}",
        post.id, post_id
    );
}

// ── helpers ───────────────────────────────────────────────────────────────

#[test]
fn test_format_thread_history() {
    let posts = vec![
        Post {
            id: test_uuid(),
            thread_id: Uuid::new_v4(),
            author: "ferb-user-proxy".to_string(),
            content: "Build a todo app".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        },
        Post {
            id: "660e8400-e29b-41d4-a716-446655440001".parse().unwrap(),
            thread_id: Uuid::new_v4(),
            author: "ferb-reviewer".to_string(),
            content: "What framework?".to_string(),
            created_at: "2026-01-01T00:01:00Z".to_string(),
        },
    ];
    let history = format_thread_history(&posts);
    assert!(history.contains("[ferb-user-proxy]: Build a todo app"));
    assert!(history.contains("[ferb-reviewer]: What framework?"));
}

#[test]
fn test_parse_agent_response_plain_json() {
    let raw = r#"{"done": false, "post": "Looks good"}"#;
    let resp = parse_agent_response(raw).unwrap();
    assert!(!resp.done);
    assert_eq!(resp.post, "Looks good");
}

#[test]
fn test_parse_agent_response_strips_markdown_fence() {
    let raw = "```json\n{\"done\": true, \"post\": \"approved\"}\n```";
    let resp = parse_agent_response(raw).unwrap();
    assert!(resp.done);
    assert_eq!(resp.post, "approved");
}
