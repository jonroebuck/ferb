use ferb_core::{SwitchboardClient, SwitchboardRunState};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn issue_json(id: &str, title: &str, status: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "title": title, "status": status })
}

fn channel_json(id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "name": name })
}

fn thread_json(id: &str, content: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "content": content, "timestamp": "2026-06-24T00:00:00Z" })
}

fn post_json(id: &str, content: &str) -> serde_json::Value {
    serde_json::json!({ "id": id, "content": content, "timestamp": "2026-06-24T00:00:00Z" })
}

#[tokio::test]
async fn test_run_start_creates_issue_and_channel() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/issues"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(issue_json("iss-1", "my task", "backlog")),
        )
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/channels"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(channel_json("ch-1", "ferb-123-my-task")),
        )
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("PATCH"))
        .and(path("/api/issues/iss-1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(issue_json("iss-1", "my task", "in_progress")),
        )
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/channels/ch-1/threads"))
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
        .create_thread(&channel.id, "Ferb run started: my task")
        .await
        .unwrap();
    assert_eq!(thread.id, "th-1");
}

#[tokio::test]
async fn test_agent_completion_posted_to_channel() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/channels/ch-1/threads/th-1/posts"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(post_json("post-1", "agent completed")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());

    let post = client
        .post_to_thread("ch-1", "th-1", "agent completed")
        .await
        .unwrap();
    assert_eq!(post.id, "post-1");
}

#[tokio::test]
async fn test_issue_transitions_to_done_on_success() {
    let server = MockServer::start().await;

    Mock::given(method("PATCH"))
        .and(path("/api/issues/iss-1"))
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

    Mock::given(method("PATCH"))
        .and(path("/api/issues/iss-1"))
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
        .and(path("/api/issues"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(issue_json("iss-2", "resume task", "backlog")),
        )
        .expect(1)
        .mount(&server)
        .await;

    // No channel creation mock — if create_channel is called, wiremock returns 404
    // which would cause an error.

    Mock::given(method("PATCH"))
        .and(path("/api/issues/iss-2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(issue_json("iss-2", "resume task", "in_progress")),
        )
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/channels/existing-ch/threads"))
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
        .create_thread(existing_channel_id, "Ferb run started: resume task")
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

#[tokio::test]
async fn test_full_lifecycle_with_agent_completions() {
    let server = MockServer::start().await;

    // Start: create issue
    Mock::given(method("POST"))
        .and(path("/api/issues"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(issue_json("iss-3", "full test", "backlog")),
        )
        .mount(&server)
        .await;

    // Start: create channel
    Mock::given(method("POST"))
        .and(path("/api/channels"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(channel_json("ch-3", "ferb-999-full-test")),
        )
        .mount(&server)
        .await;

    // Start: transition to in_progress, then later to done
    Mock::given(method("PATCH"))
        .and(path("/api/issues/iss-3"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(issue_json("iss-3", "full test", "done")),
        )
        .mount(&server)
        .await;

    // Start: create thread
    Mock::given(method("POST"))
        .and(path("/api/channels/ch-3/threads"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(thread_json("th-3", "Ferb run started: full test")),
        )
        .mount(&server)
        .await;

    // Agent completions + final summary: post to thread
    Mock::given(method("POST"))
        .and(path("/api/channels/ch-3/threads/th-3/posts"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(post_json("post-x", "update")),
        )
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
        .create_thread(&channel.id, "Ferb run started: full test")
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
            &run_state.channel_id,
            &run_state.thread_id,
            "Agent: ferb-reviewer | Task: define-goal | Status: Done",
        )
        .await
        .unwrap();

    client
        .post_to_thread(
            &run_state.channel_id,
            &run_state.thread_id,
            "Agent: ferb-worker | Task: make-plan | Status: ReadyForReview",
        )
        .await
        .unwrap();

    // 3. Finish success
    client
        .post_to_thread(
            &run_state.channel_id,
            &run_state.thread_id,
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
