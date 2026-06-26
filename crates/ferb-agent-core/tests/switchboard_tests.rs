use ferb_agent_core::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn channel_schema_json() -> serde_json::Value {
    serde_json::json!({
        "resource": "channels",
        "required": ["name", "description"],
        "optional": []
    })
}

fn uuid_str() -> &'static str {
    "550e8400-e29b-41d4-a716-446655440000"
}

fn test_uuid() -> Uuid {
    uuid_str().parse().unwrap()
}

#[tokio::test]
async fn test_workflow_yaml_parsing() {
    let yaml = r#"
name: test-workflow
description: A test workflow
channels:
  - name: general
    threads:
      - name: progress
      - name: questions
cards:
  - title: define-goal
    agents: [ferb-reviewer, ferb-user-proxy]
  - title: implementation
    agents: [ferb-worker, ferb-reviewer]
agents:
  - name: ferb-reviewer
    role: Reviews work
  - name: ferb-worker
    role: Implements solutions
"#;
    let wf = parse_workflow(yaml).unwrap();
    assert_eq!(wf.name, "test-workflow");
    assert_eq!(wf.channels.len(), 1);
    assert_eq!(wf.channels[0].threads.len(), 2);
    assert_eq!(wf.cards.len(), 2);
    assert_eq!(wf.cards[0].agents, vec!["ferb-reviewer", "ferb-user-proxy"]);
    assert_eq!(wf.agents.len(), 2);
    assert!(!wf.is_bootstrap());
}

#[tokio::test]
async fn test_bootstrap_workflow_parsing() {
    let yaml = r#"
name: default
description: Bootstrap workflow

steps:
  - name: fetch-workflow
    agent: ferb-moderator
    task: fetch the workflow
  - name: setup-channels
    agent: ferb-moderator
    task: create channels
    depends_on: fetch-workflow
  - name: handoff
    task: start target workflow
    depends_on:
      - setup-channels
"#;
    let wf = parse_workflow(yaml).unwrap();
    assert_eq!(wf.name, "default");
    assert!(wf.is_bootstrap());
    assert_eq!(wf.steps.len(), 3);
    assert_eq!(wf.steps[0].agent, Some("ferb-moderator".to_string()));
    assert!(wf.steps[2].agent.is_none());

    match &wf.steps[2].depends_on {
        Some(DependsOn::Multiple(deps)) => assert_eq!(deps, &["setup-channels"]),
        _ => panic!("expected multiple deps"),
    }
}

#[tokio::test]
async fn test_get_card() {
    let server = MockServer::start().await;
    let id = test_uuid();

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/issues/{}", id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": id,
            "title": "test card",
            "status": "backlog",
            "assigned_agents": ["ferb-reviewer"]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let issue = client.get_issue(id).await.unwrap();
    assert_eq!(issue.title, "test card");
    assert_eq!(issue.status, IssueStatus::Backlog);
    assert_eq!(issue.assigned_agents, vec!["ferb-reviewer"]);
}

#[tokio::test]
async fn test_update_card_status() {
    let server = MockServer::start().await;
    let id = test_uuid();
    let event_id: Uuid = "660e8400-e29b-41d4-a716-446655440001".parse().unwrap();

    Mock::given(method("PATCH"))
        .and(path(format!("/api/v1/issues/{}", id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": event_id,
            "issue_id": id,
            "agent": "system",
            "content": "Status changed to in_progress",
            "timestamp": "2026-06-25T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let event = client
        .update_issue_status(id, &IssueStatus::InProgress)
        .await
        .unwrap();
    assert_eq!(event.issue_id, id);
}

#[tokio::test]
async fn test_post_card_comment() {
    let server = MockServer::start().await;
    let id = test_uuid();
    let event_id: Uuid = "770e8400-e29b-41d4-a716-446655440002".parse().unwrap();

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/issues/{}/comments", id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": event_id,
            "issue_id": id,
            "agent": "ferb-reviewer",
            "content": "looks good",
            "timestamp": "2026-06-25T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let event = client
        .post_issue_comment(id, "ferb-reviewer", "looks good")
        .await
        .unwrap();
    assert_eq!(event.content, "looks good");
}

#[tokio::test]
async fn test_list_card_questions() {
    let server = MockServer::start().await;
    let id = test_uuid();
    let q_id: Uuid = "880e8400-e29b-41d4-a716-446655440003".parse().unwrap();

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/issues/{}/questions", id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": q_id,
                "text": "What color?",
                "asked_by": "ferb-reviewer",
                "answer": null,
                "answered_by": null
            }
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let questions = client.list_questions(id).await.unwrap();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].text, "What color?");
    assert!(questions[0].answer.is_none());
}

#[tokio::test]
async fn test_post_question() {
    let server = MockServer::start().await;
    let id = test_uuid();
    let q_id: Uuid = "990e8400-e29b-41d4-a716-446655440004".parse().unwrap();

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/issues/{}/questions", id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": q_id,
            "text": "What framework?",
            "asked_by": "ferb-reviewer"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let q = client
        .post_question(id, "What framework?", "ferb-reviewer")
        .await
        .unwrap();
    assert_eq!(q.text, "What framework?");
}

#[tokio::test]
async fn test_answer_question() {
    let server = MockServer::start().await;
    let id = test_uuid();
    let q_id: Uuid = "aa0e8400-e29b-41d4-a716-446655440005".parse().unwrap();

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/issues/{}/questions/{}/answers", id, q_id)))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    client
        .answer_question(id, q_id, "React", "ferb-user-proxy")
        .await
        .unwrap();
}

#[tokio::test]
async fn test_create_channel_and_thread() {
    let server = MockServer::start().await;
    let ch_id = test_uuid();
    let th_id: Uuid = "bb0e8400-e29b-41d4-a716-446655440006".parse().unwrap();

    Mock::given(method("GET"))
        .and(path("/api/v1/schema/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(channel_schema_json()))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": ch_id,
            "name": "general",
            "threads": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/channels/{}/threads", ch_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": th_id,
            "channel_id": ch_id,
            "title": "progress"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let channel = client.create_channel("general", "General channel").await.unwrap();
    assert_eq!(channel.name, "general");

    let thread = client.create_thread(channel.id, "progress").await.unwrap();
    assert_eq!(thread.title, "progress");
    assert_eq!(thread.channel_id, ch_id);
}

#[tokio::test]
async fn test_post_to_thread_and_list() {
    let server = MockServer::start().await;
    let th_id = test_uuid();
    let post_id: Uuid = "cc0e8400-e29b-41d4-a716-446655440007".parse().unwrap();

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/threads/{}/posts", th_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": post_id,
            "thread_id": th_id,
            "content": "hello",
            "author": "ferb-reviewer",
            "timestamp": "2026-06-25T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/threads/{}/posts", th_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {
                "id": post_id,
                "thread_id": th_id,
                "content": "hello",
                "author": "ferb-reviewer",
                "timestamp": "2026-06-25T00:00:00Z"
            }
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());

    let post = client
        .post_to_thread(th_id, "ferb-reviewer", "hello")
        .await
        .unwrap();
    assert_eq!(post.content, "hello");

    let posts = client.list_thread_posts(th_id).await.unwrap();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].author, "ferb-reviewer");
}

#[tokio::test]
async fn test_create_issue_with_agents() {
    let server = MockServer::start().await;
    let id = test_uuid();

    Mock::given(method("POST"))
        .and(path("/api/v1/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": id,
            "title": "define-goal",
            "status": "backlog",
            "assigned_agents": ["ferb-reviewer", "ferb-user-proxy"]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let agents = vec!["ferb-reviewer".to_string(), "ferb-user-proxy".to_string()];
    let issue = client.create_issue("define-goal", &agents).await.unwrap();
    assert_eq!(issue.assigned_agents.len(), 2);
}

#[tokio::test]
async fn test_agent_response_noop() {
    let resp = AgentResponse::noop("test-card");
    assert!(!resp.done);
    assert_eq!(resp.card_id, "test-card");
    assert!(resp.questions.is_empty());
    assert!(resp.answers.is_empty());
    assert!(resp.artifacts.is_empty());
    assert!(resp.message.is_empty());
}

#[tokio::test]
async fn test_get_schema() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/schema/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(channel_schema_json()))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let schema = client.get_schema("channels").await.unwrap();
    assert_eq!(schema.resource, "channels");
    assert_eq!(schema.required, vec!["name", "description"]);
    assert!(schema.optional.is_empty());
}

#[tokio::test]
async fn test_create_channel_formats_payload_from_schema() {
    let server = MockServer::start().await;
    let ch_id = test_uuid();

    Mock::given(method("GET"))
        .and(path("/api/v1/schema/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(channel_schema_json()))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/channels"))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "name": "dev",
            "description": "Development channel"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": ch_id,
            "name": "dev",
            "threads": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = SwitchboardClient::new(&server.uri());
    let channel = client.create_channel("dev", "Development channel").await.unwrap();
    assert_eq!(channel.name, "dev");
}

#[test]
fn test_validate_required_fields_all_present() {
    let schema = CreateSchema {
        resource: "channels".to_string(),
        required: vec!["name".to_string(), "description".to_string()],
        optional: vec![],
    };
    let mut fields = serde_json::Map::new();
    fields.insert("name".to_string(), "test".into());
    fields.insert("description".to_string(), "Test channel".into());
    assert!(validate_required_fields(&schema, &fields).is_ok());
}

#[test]
fn test_missing_required_field_caught_before_sending() {
    let schema = CreateSchema {
        resource: "channels".to_string(),
        required: vec!["name".to_string(), "description".to_string()],
        optional: vec![],
    };
    let mut fields = serde_json::Map::new();
    fields.insert("name".to_string(), "test".into());
    // description deliberately omitted
    let err = validate_required_fields(&schema, &fields).unwrap_err();
    assert!(err.to_string().contains("description"));
    assert!(err.to_string().contains("channels"));
}
