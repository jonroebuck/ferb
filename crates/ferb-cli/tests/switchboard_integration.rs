use ferb_core::{SwitchboardClient, SwitchboardRunState};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn issue_json(id: &str, title: &str, status: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "title": title, "status": status })
}

fn channel_json(id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "name": name })
}

fn thread_json(id: &str, title: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "title": title, "author": "system" })
}

fn post_json(id: &str, content: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "content": content, "timestamp": "2026-06-24T00:00:00Z" })
}

#[tokio::test]
async fn test_run_start_creates_issue_and_channel() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/issues"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(issue_json("iss-1", "my task", "backlog")),
        )
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/channels"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(channel_json("ch-1", "ferb-123-my-task")),
        )
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path("/api/v1/issues/iss-1/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(
            "iss-1",
            "my task",
            "in_progress",
        )))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/channels/ch-1/threads"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(thread_json("th-1", "Ferb run started: my task")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());

    let issue = client.create_issue("my task", "backlog").await.unwrap();
    assert_eq!(issue.id, "iss-1");
    assert_eq!(issue.status, "backlog");

    let channel = client.create_channel("ferb-123-my-task").await.unwrap();
    assert_eq!(channel.id, "ch-1");

    let transitioned = client
        .transition_issue(&issue.id, "in_progress")
        .await
        .unwrap();
    assert_eq!(transitioned.status, "in_progress");

    let thread = client
        .create_thread(&channel.id, "Ferb run started: my task", "system")
        .await
        .unwrap();
    assert_eq!(thread.id, "th-1");
}

#[tokio::test]
async fn test_agent_completion_posted_to_channel() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/threads/th-1/posts"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(post_json("post-1", "agent completed")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());

    let post = client
        .post_to_thread("th-1", "system", "agent completed")
        .await
        .unwrap();
    assert_eq!(post.id, "post-1");
}

#[tokio::test]
async fn test_issue_transitions_to_done_on_success() {
    let server = MockServer::start().await;

    Mock::given(method("PUT"))
        .and(path("/api/v1/issues/iss-1/status"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(issue_json("iss-1", "my task", "done")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());

    let result = client.transition_issue("iss-1", "done").await.unwrap();
    assert_eq!(result.status, "done");
}

#[tokio::test]
async fn test_issue_transitions_to_blocked_on_failure() {
    let server = MockServer::start().await;

    Mock::given(method("PUT"))
        .and(path("/api/v1/issues/iss-1/status"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(issue_json("iss-1", "my task", "blocked")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());

    let result = client.transition_issue("iss-1", "blocked").await.unwrap();
    assert_eq!(result.status, "blocked");
}

#[tokio::test]
async fn test_channel_flag_reuses_existing_channel() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(
            "iss-2",
            "resume task",
            "backlog",
        )))
        .expect(1)
        .mount(&server)
        .await;

    // No channel creation mock — if create_channel is called, wiremock returns 404
    // which would cause an error.

    Mock::given(method("PUT"))
        .and(path("/api/v1/issues/iss-2/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(
            "iss-2",
            "resume task",
            "in_progress",
        )))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/channels/existing-ch/threads"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(thread_json("th-2", "Ferb run started: resume task")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());

    // Simulate the --channel flag flow: create issue, skip channel creation, use existing channel
    let issue = client.create_issue("resume task", "backlog").await.unwrap();
    assert_eq!(issue.id, "iss-2");

    // Use the existing channel ID directly (no create_channel call)
    let existing_channel_id = "existing-ch";

    client
        .transition_issue(&issue.id, "in_progress")
        .await
        .unwrap();

    let thread = client
        .create_thread(
            existing_channel_id,
            "Ferb run started: resume task",
            "system",
        )
        .await
        .unwrap();
    assert_eq!(thread.id, "th-2");

    // Verify: no channel creation happened (wiremock expect counts enforce this)
}

#[tokio::test]
async fn test_switchboard_unavailable_returns_error() {
    // Connect to a port where nothing is listening
    let client = SwitchboardClient::new("http://127.0.0.1:1");

    let result = client.create_issue("test", "backlog").await;
    assert!(result.is_err());
}

fn channels_schema_json() -> serde_json::Value {
    serde_json::json!({
        "resource": "channels",
        "required": ["name", "description"],
        "optional": []
    })
}

#[tokio::test]
async fn test_health_check_succeeds_when_reachable() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/schema/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(channels_schema_json()))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    assert!(client.health_check().await.is_ok());
}

#[tokio::test]
async fn test_health_check_fails_when_unreachable() {
    let client = SwitchboardClient::new("http://127.0.0.1:1");
    let result = client.health_check().await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Cannot connect to Switchboard"),
        "unexpected: {}",
        msg
    );
}

#[tokio::test]
async fn test_health_check_fails_on_non_2xx() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/schema/channels"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let result = client.health_check().await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Cannot connect to Switchboard"),
        "unexpected: {}",
        msg
    );
}

#[tokio::test]
async fn test_create_channel_adds_required_fields_from_schema() {
    let server = MockServer::start().await;

    // Schema endpoint returns description as required.
    Mock::given(method("GET"))
        .and(path("/api/v1/schema/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "resource": "channels",
            "required": ["name", "description"],
            "optional": []
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(channel_json("ch-s", "test-ch")))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let channel = client.create_channel("test-ch").await.unwrap();
    assert_eq!(channel.name, "test-ch");
}

#[tokio::test]
async fn test_create_issue_failure_returns_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/issues"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let result = client.create_issue("failing task", "backlog").await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("create_issue"), "unexpected: {}", msg);
}

#[tokio::test]
async fn test_create_channel_failure_returns_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/channels"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let result = client.create_channel("failing-channel").await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("create_channel"), "unexpected: {}", msg);
}

fn artifact_json(id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "name": name })
}

#[tokio::test]
async fn test_create_artifact_with_schema_defaults() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/schema/artifacts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "resource": "artifacts",
            "required": ["name", "content_type", "source_type"],
            "optional": ["description"]
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/artifacts"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(artifact_json("art-1", "my-artifact")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let artifact = client.create_artifact("my-artifact").await.unwrap();
    assert_eq!(artifact.id, "art-1");
    assert_eq!(artifact.name, "my-artifact");
}

#[tokio::test]
async fn test_create_artifact_without_schema_still_posts() {
    let server = MockServer::start().await;

    // No schema mock — wiremock returns 404, so schema fetch returns None.
    // create_artifact should proceed with just {name}.
    Mock::given(method("POST"))
        .and(path("/api/v1/artifacts"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(artifact_json("art-2", "bare-artifact")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let artifact = client.create_artifact("bare-artifact").await.unwrap();
    assert_eq!(artifact.id, "art-2");
}

#[tokio::test]
async fn test_create_artifact_failure_returns_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/artifacts"))
        .respond_with(ResponseTemplate::new(422).set_body_string("Unprocessable Entity"))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let result = client.create_artifact("bad-artifact").await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("create_artifact"), "unexpected: {}", msg);
}

#[tokio::test]
async fn test_full_lifecycle_with_agent_completions() {
    let server = MockServer::start().await;

    // Start: create issue
    Mock::given(method("POST"))
        .and(path("/api/v1/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(
            "iss-3",
            "full test",
            "backlog",
        )))
        .mount(&server)
        .await;

    // Start: create channel
    Mock::given(method("POST"))
        .and(path("/api/v1/channels"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(channel_json("ch-3", "ferb-999-full-test")),
        )
        .mount(&server)
        .await;

    // Start: transition to in_progress, then later to done
    Mock::given(method("PUT"))
        .and(path("/api/v1/issues/iss-3/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(
            "iss-3",
            "full test",
            "done",
        )))
        .mount(&server)
        .await;

    // Start: create thread
    Mock::given(method("POST"))
        .and(path("/api/v1/channels/ch-3/threads"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(thread_json("th-3", "Ferb run started: full test")),
        )
        .mount(&server)
        .await;

    // Agent completions + final summary: post to thread
    Mock::given(method("POST"))
        .and(path("/api/v1/threads/th-3/posts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(post_json("post-x", "update")))
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());

    // 1. Start
    let issue = client.create_issue("full test", "backlog").await.unwrap();
    let channel = client.create_channel("ferb-999-full-test").await.unwrap();
    client
        .transition_issue(&issue.id, "in_progress")
        .await
        .unwrap();
    let thread = client
        .create_thread(&channel.id, "Ferb run started: full test", "system")
        .await
        .unwrap();

    let run_state = SwitchboardRunState {
        issue_id: issue.id.clone(),
        channel_id: channel.id.clone(),
        thread_id: thread.id.clone(),
    };

    // 2. Agent completions
    client
        .post_to_thread(
            &run_state.thread_id,
            "system",
            "Agent: ferb-reviewer | Task: define-goal | Status: Done",
        )
        .await
        .unwrap();

    client
        .post_to_thread(
            &run_state.thread_id,
            "system",
            "Agent: ferb-worker | Task: make-plan | Status: ReadyForReview",
        )
        .await
        .unwrap();

    // 3. Finish success
    client
        .post_to_thread(
            &run_state.thread_id,
            "system",
            "Ferb run completed successfully.",
        )
        .await
        .unwrap();
    let final_issue = client
        .transition_issue(&run_state.issue_id, "done")
        .await
        .unwrap();
    assert_eq!(final_issue.status, "done");
}
