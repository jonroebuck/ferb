use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ferb_agent_core::{CardContext, CardWorkflow, FerbAgent, RunState, SwitchboardClient, Uuid};
use ferb_approver::Approver;
use ferb_core::{FerbState, KanbanBoard, KanbanTask, TaskStatus};
use ferb_moderator::Moderator;
use ferb_reviewer::Reviewer;
use ferb_user_proxy::UserProxy;
use ferb_worker::Worker;
use serde::Deserialize;
use std::collections::HashMap;

use crate::FerbConfig;

const MAX_PASSES: usize = 10;
const MAX_DEFINE_GOAL_ITERATIONS: usize = 10;

fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~/") || path == "~" {
        let home = if cfg!(windows) {
            std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOMEPATH"))
                .unwrap_or_else(|_| "C:\\Users\\Default".to_string())
        } else {
            std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
        };
        PathBuf::from(home).join(&path[2..])
    } else {
        PathBuf::from(path)
    }
}

#[derive(Debug, Deserialize)]
struct WorkflowTaskDef {
    id: String,
    name: String,
    agent: String,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    inputs: Vec<String>,
    #[serde(default)]
    reviews: Option<String>,
    #[serde(default)]
    approves: Option<String>,
    #[serde(default)]
    max_iterations: usize,
    #[serde(default)]
    success_criteria: Vec<String>,
    #[serde(default = "default_pass_budget")]
    pass_budget: usize,
}

fn default_pass_budget() -> usize {
    3
}

#[derive(Debug, Deserialize)]
struct TaskWorkflowDef {
    #[allow(dead_code)]
    workflow: String,
    tasks: Vec<WorkflowTaskDef>,
}

fn load_kanban_from_str(content: &str) -> anyhow::Result<KanbanBoard> {
    let def: TaskWorkflowDef = serde_yaml::from_str(content)?;

    let tasks = def
        .tasks
        .into_iter()
        .map(|t| KanbanTask {
            id: t.id,
            name: t.name,
            agent: t.agent,
            prompt: t.prompt,
            status: TaskStatus::Pending,
            inputs: t.inputs,
            reviews: t.reviews,
            approves: t.approves,
            max_iterations: t.max_iterations,
            iterations_used: 0,
            questions: vec![],
            comments: vec![],
            success_criteria: t.success_criteria,
            pass_budget: t.pass_budget,
        })
        .collect();

    Ok(KanbanBoard { tasks })
}

// ── Card-based workflow pipeline ───────────────────────────────────────────

fn is_card_workflow(content: &str) -> bool {
    serde_yaml::from_str::<serde_json::Value>(content)
        .map(|v| v.get("cards").is_some())
        .unwrap_or(false)
}

fn card_prompts_dir() -> std::path::PathBuf {
    std::env::var("FERB_PROMPTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("./prompts"))
}

fn load_card_prompt(card_title: &str) -> Option<String> {
    let path = card_prompts_dir().join(format!("{}.md", card_title));
    std::fs::read_to_string(&path).ok()
}

async fn build_input_context(
    sb: &SwitchboardClient,
    inputs: &[String],
    output_threads: &HashMap<String, Uuid>,
) -> String {
    if inputs.is_empty() {
        return String::new();
    }
    let mut ctx = String::from("## Context from previous cards\n\n");
    for input_name in inputs {
        if let Some(&tid) = output_threads.get(input_name) {
            let posts = sb.list_posts(tid).await.unwrap_or_default();
            ctx.push_str(&format!("### {}\n", input_name));
            for post in &posts {
                ctx.push_str(&format!("[{}]: {}\n", post.author, post.content));
            }
            ctx.push('\n');
        }
    }
    ctx
}

/// Run the define-goal interactive phase and post the confirmed goal to each
/// named output thread.
async fn run_define_goal_card(
    goal: &str,
    run_state: &mut RunState,
    sb: &SwitchboardClient,
    reviewer: &Reviewer,
    outputs: &[String],
) -> anyhow::Result<()> {
    let board = KanbanBoard {
        tasks: vec![KanbanTask {
            id: "define-goal".to_string(),
            name: "Define Goal".to_string(),
            agent: "ferb-user-proxy".to_string(),
            prompt: None,
            status: TaskStatus::Pending,
            inputs: vec![],
            reviews: None,
            approves: None,
            max_iterations: 10,
            iterations_used: 0,
            questions: vec![],
            comments: vec![],
            success_criteria: vec![],
            pass_budget: 3,
        }],
    };
    let mut state = FerbState::new(board);
    state
        .channel_ids
        .insert("general".to_string(), run_state.channel_id.to_string());

    run_define_goal_phase(&mut state, sb, reviewer, goal).await?;

    // Post the confirmed goal JSON to each output thread.
    if let Some(artifact) = state.get_artifact("define-goal") {
        let content = serde_json::to_string_pretty(artifact).unwrap_or_default();
        if let Some(refined) = artifact.get("refined_goal").and_then(|v| v.as_str()) {
            run_state.confirmed_goal = Some(refined.to_string());
        }
        for output_name in outputs {
            if let Some(&tid) = run_state.output_threads.get(output_name) {
                if let Err(e) = sb.post_to_thread(tid, "ferb-reviewer", &content).await {
                    eprintln!("[warn] Failed to post goal to '{}' thread: {}", output_name, e);
                }
            }
        }
    }
    Ok(())
}

async fn run_card_pipeline(
    workflow: &CardWorkflow,
    goal: &str,
    run_state: &mut RunState,
    sb: &SwitchboardClient,
    tramway: &ferb_core::TramwayClient,
    worker: &Worker,
    reviewer: &Reviewer,
) -> anyhow::Result<()> {
    for card in &workflow.cards {
        println!("\n=== {} ===\n", card.title);

        // Create output threads before running so inputs from later cards can be wired up.
        for output_name in &card.outputs {
            let thread = sb
                .create_thread(run_state.channel_id, output_name)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("Failed to create output thread '{}': {}", output_name, e)
                })?;
            run_state.output_threads.insert(output_name.clone(), thread.id);
        }

        // define-goal runs an interactive conversation; handled separately.
        if card.title == "define-goal" {
            run_define_goal_card(goal, run_state, sb, reviewer, &card.outputs).await?;
            continue;
        }

        // Build context string from all input threads.
        let input_context =
            build_input_context(sb, &card.inputs, &run_state.output_threads).await;

        // Load a card-specific system prompt (e.g. prompts/develop-plan.md).
        let prompt = load_card_prompt(&card.title);

        // Create a Switchboard issue to track this card.
        let card_issue = sb.create_issue(&card.title).await.map_err(|e| {
            anyhow::anyhow!("Failed to create issue for card '{}': {}", card.title, e)
        })?;

        // Create a working thread for the card.
        let card_thread = sb
            .create_thread(run_state.channel_id, &card.title)
            .await
            .map_err(|e| {
                anyhow::anyhow!("Failed to create thread for card '{}': {}", card.title, e)
            })?;

        let context = CardContext {
            card: card_issue,
            thread_id: card_thread.id,
            channel_id: run_state.channel_id,
            posts: vec![],
            input_context,
            prompt,
        };

        let primary_agent = card.agents.first().map(String::as_str).unwrap_or("ferb-worker");
        let resp = match primary_agent {
            "ferb-worker" => worker.run(context, tramway).await?,
            "ferb-reviewer" => reviewer.run(context, tramway).await?,
            unknown => {
                eprintln!("[warn] Unknown agent '{}' for card '{}', skipping", unknown, card.title);
                continue;
            }
        };

        eprintln!(
            "[info] Card '{}' done={} post={}",
            card.title,
            resp.done,
            &resp.post[..resp.post.len().min(120)]
        );

        // Post result to each output thread.
        for output_name in &card.outputs {
            if let Some(&tid) = run_state.output_threads.get(output_name) {
                if let Err(e) = sb.post_to_thread(tid, primary_agent, &resp.post).await {
                    eprintln!("[warn] Failed to post to output thread '{}': {}", output_name, e);
                }
            }
        }

        // Progress summary to the main thread.
        let summary = format!("Card '{}' completed.", card.title);
        if let Err(e) = sb.post_to_thread(run_state.thread_id, "system", &summary).await {
            eprintln!("[warn] Failed to post progress: {}", e);
        }
    }
    Ok(())
}

async fn run_card_workflow_task(
    goal: &str,
    channel_id: Option<&str>,
    wf_content: &str,
    config: &FerbConfig,
) -> anyhow::Result<()> {
    let workflow: CardWorkflow = serde_yaml::from_str(wf_content)
        .map_err(|e| anyhow::anyhow!("Failed to parse card workflow: {}", e))?;

    let sb_url = &config.switchboard.url;
    let tw_url = &config.tramway.url;
    let model = &config.tramway.model;

    let sb = SwitchboardClient::new(sb_url);
    let reviewer = Reviewer::new(sb_url, tw_url, model);
    let worker = Worker::new(sb_url, tw_url, model);
    let tramway = ferb_core::TramwayClient::new(tw_url, model);

    sb.health_check().await.map_err(|e| {
        anyhow::anyhow!("Error: {}\nRun 'ferb up' to start all required services.", e)
    })?;

    println!("\n=== Ferb ===\n");

    let issue = sb.create_issue(goal).await.map_err(|e| {
        anyhow::anyhow!("Switchboard setup failed — create_issue: {}", e)
    })?;

    let ch_id_str = if let Some(id) = channel_id {
        id.to_string()
    } else {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let slug = slugify(goal);
        let name = format!("ferb-{}-{}", ts, slug);
        sb.create_channel(&name)
            .await
            .map(|ch| ch.id.to_string())
            .map_err(|e| anyhow::anyhow!("Switchboard setup failed — create_channel: {}", e))?
    };

    let ch_uuid: Uuid = ch_id_str
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid channel_id: {}", ch_id_str))?;

    if let Err(e) = sb.update_issue_status(issue.id, "in_progress").await {
        eprintln!("[warn] Switchboard: failed to transition issue: {}", e);
    }

    let progress_thread = sb
        .create_thread(ch_uuid, &format!("Ferb: {}", goal))
        .await
        .map_err(|e| anyhow::anyhow!("Switchboard setup failed — create_thread: {}", e))?;

    let mut run_state = RunState {
        channel_id: ch_uuid,
        thread_id: progress_thread.id,
        issue_id: issue.id,
        output_threads: HashMap::new(),
        confirmed_goal: None,
    };

    let result =
        run_card_pipeline(&workflow, goal, &mut run_state, &sb, &tramway, &worker, &reviewer)
            .await;

    match &result {
        Ok(()) => {
            let _ = sb
                .post_to_thread(run_state.thread_id, "system", "Workflow completed successfully.")
                .await;
            if let Err(e) = sb.update_issue_status(run_state.issue_id, "done").await {
                eprintln!("[warn] Switchboard: failed to transition issue to done: {}", e);
            }
        }
        Err(e) => {
            let msg = format!("Workflow failed: {}", e);
            let _ = sb.post_to_thread(run_state.thread_id, "system", &msg).await;
            if let Err(e2) = sb.update_issue_status(run_state.issue_id, "blocked").await {
                eprintln!("[warn] Switchboard: failed to transition issue to blocked: {}", e2);
            }
        }
    }

    result
}

// ── Task-based pipeline (legacy default.yaml) ─────────────────────────────

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn print_kanban(state: &FerbState) {
    println!("\n  Kanban:");
    for task in &state.kanban_board.tasks {
        let q_count = task.questions.len();
        let unanswered = task
            .questions
            .iter()
            .filter(|q| q.status == ferb_core::QuestionStatus::Unanswered)
            .count();
        println!(
            "    [{:>16?}] {} (iter {}/{}) Q:{}/{}",
            task.status, task.name, task.iterations_used, task.max_iterations, unanswered, q_count,
        );
    }
    println!();
}

async fn switchboard_start(
    sb: &SwitchboardClient,
    title: &str,
    channel_id: Option<&str>,
) -> anyhow::Result<ferb_core::SwitchboardRunState> {
    let issue = sb.create_issue(title).await.map_err(|e| {
        anyhow::anyhow!("Switchboard setup failed — create_issue: {}", e)
    })?;

    let ch_id_str = if let Some(id) = channel_id {
        id.to_string()
    } else {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let slug = slugify(title);
        let name = format!("ferb-{}-{}", ts, slug);
        sb.create_channel(&name).await.map(|ch| ch.id.to_string()).map_err(|e| {
            anyhow::anyhow!("Switchboard setup failed — create_channel: {}", e)
        })?
    };

    if let Err(e) = sb.update_issue_status(issue.id, "in_progress").await {
        eprintln!("[warn] Switchboard: failed to transition issue: {}", e);
    }

    let ch_uuid: Uuid = ch_id_str
        .parse()
        .map_err(|_| anyhow::anyhow!("Switchboard setup failed — invalid channel_id: {}", ch_id_str))?;

    let thread_id = sb
        .create_thread(ch_uuid, &format!("Ferb run started: {}", title))
        .await
        .map(|t| t.id.to_string())
        .map_err(|e| anyhow::anyhow!("Switchboard setup failed — create_thread: {}", e))?;

    Ok(ferb_core::SwitchboardRunState {
        issue_id: issue.id.to_string(),
        channel_id: ch_id_str,
        thread_id,
    })
}

async fn switchboard_post_agent_completion(
    sb: &SwitchboardClient,
    run: &ferb_core::SwitchboardRunState,
    agent: &str,
    task_id: &str,
    task_name: &str,
    status: &TaskStatus,
) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let content = format!(
        "[{}] Agent: {} | Task: {} ({}) | Status: {:?}",
        ts, agent, task_name, task_id, status
    );
    if let Ok(tid) = run.thread_id.parse::<Uuid>() {
        if let Err(e) = sb.post_to_thread(tid, "system", &content).await {
            eprintln!("[warn] Switchboard: failed to post agent completion: {}", e);
        }
    }
}

async fn switchboard_finish_success(
    sb: &SwitchboardClient,
    run: &ferb_core::SwitchboardRunState,
    state: &FerbState,
) {
    let mut summary = String::from("Ferb run completed successfully.\n\nCompleted tasks:");
    for task in &state.kanban_board.tasks {
        summary.push_str(&format!("\n- {} [{:?}]", task.name, task.status));
    }

    if let Ok(tid) = run.thread_id.parse::<Uuid>() {
        if let Err(e) = sb.post_to_thread(tid, "system", &summary).await {
            eprintln!("[warn] Switchboard: failed to post completion summary: {}", e);
        }
    }

    if let Ok(iid) = run.issue_id.parse::<Uuid>() {
        if let Err(e) = sb.update_issue_status(iid, "done").await {
            eprintln!("[warn] Switchboard: failed to transition issue to done: {}", e);
        }
    }
}

async fn switchboard_finish_failure(
    sb: &SwitchboardClient,
    run: &ferb_core::SwitchboardRunState,
    error: &str,
) {
    let content = format!("Ferb run failed: {}", error);
    if let Ok(tid) = run.thread_id.parse::<Uuid>() {
        if let Err(e) = sb.post_to_thread(tid, "system", &content).await {
            eprintln!("[warn] Switchboard: failed to post error details: {}", e);
        }
    }

    if let Ok(iid) = run.issue_id.parse::<Uuid>() {
        if let Err(e) = sb.update_issue_status(iid, "blocked").await {
            eprintln!("[warn] Switchboard: failed to transition issue to blocked: {}", e);
        }
    }
}

// ── Define-Goal Phase ──────────────────────────────────────────────────────

/// Parse a labeled post envelope: `{"type": "...", "content": "..."}`.
/// Returns (type_str, content_str). Falls back to ("status", raw) for non-JSON.
#[allow(dead_code)]
fn parse_labeled_post_content(content: &str) -> (String, String) {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
        let t = val["type"].as_str().unwrap_or("status").to_string();
        let c = val["content"].as_str().unwrap_or(content).to_string();
        return (t, c);
    }
    ("status".to_string(), content.to_string())
}

/// Post the initial goal to the define-goal thread.
/// If the first attempt fails, recreates the thread and retries once.
/// Returns the active thread_id (may differ from the input if a new thread was created).
/// Returns an error if the goal cannot be posted — the reviewer must not run against an empty thread.
async fn post_initial_goal_with_retry(
    sb: &SwitchboardClient,
    goal: &str,
    state: &mut FerbState,
    channel_id: &str,
    thread_id: String,
) -> anyhow::Result<String> {
    println!(
        "[trace] post_initial_goal_with_retry: attempting post to channel_id={} thread_id={}",
        channel_id, thread_id
    );
    if let Ok(tid) = thread_id.parse::<Uuid>() {
        if sb.post_to_thread(tid, "ferb-user-proxy", goal).await.is_ok() {
            println!("[trace] post_initial_goal_with_retry: initial post succeeded thread_id={}", thread_id);
            return Ok(thread_id);
        }
    }

    eprintln!("[warn] Initial post to define-goal thread failed, retrying with a fresh thread...");
    let ch_uuid: Uuid = channel_id
        .parse()
        .map_err(|_| anyhow::anyhow!("Could not create define-goal thread on retry: invalid channel_id={}", channel_id))?;
    let new_thread = sb
        .create_thread(ch_uuid, &format!("Define Goal: {}", goal))
        .await
        .map_err(|e| anyhow::anyhow!("Could not create define-goal thread on retry: {}", e))?;

    let new_thread_id = new_thread.id.to_string();
    println!(
        "[trace] post_initial_goal_with_retry: retry thread created thread_id={}",
        new_thread_id
    );
    state
        .thread_ids
        .insert("define-goal".to_string(), new_thread_id.clone());

    println!(
        "[trace] post_initial_goal_with_retry: posting to retry thread channel_id={} thread_id={}",
        channel_id, new_thread_id
    );
    sb.post_to_thread(new_thread.id, "ferb-user-proxy", goal)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to post goal to define-goal thread after retry: {}. \
                 Aborting — the reviewer cannot run against an empty thread.",
                e
            )
        })?;

    println!("[trace] post_initial_goal_with_retry: retry post succeeded thread_id={}", new_thread_id);
    Ok(new_thread_id)
}

/// Run the define-goal conversation between the reviewer and the user.
///
/// - Creates a "general" Switchboard channel and a "define-goal" thread.
/// - Posts the raw goal as the first message from ferb-user-proxy.
/// - Loops: reviewer analyzes thread → posts question or summary → user responds.
/// - On user confirmation: stores the goal artifact, marks the card Done.
/// - If Switchboard is unavailable: falls back to storing the raw goal directly.
async fn run_define_goal_phase(
    state: &mut FerbState,
    sb: &SwitchboardClient,
    reviewer: &Reviewer,
    goal: &str,
) -> anyhow::Result<()> {
    // Try to set up the Switchboard channel + thread.
    let (channel_id, thread_id) = match setup_define_goal_channel(sb, goal, state).await {
        Some(ids) => ids,
        None => {
            // Switchboard unavailable — store the raw goal and continue.
            store_raw_goal(state, goal);
            return Ok(());
        }
    };
    println!(
        "[trace] run_define_goal_phase: received channel_id={} thread_id={}",
        channel_id, thread_id
    );

    // Post the initial task description — must succeed before the reviewer runs.
    println!(
        "[trace] run_define_goal_phase: calling post_initial_goal_with_retry with channel_id={} thread_id={}",
        channel_id, thread_id
    );
    let thread_id =
        post_initial_goal_with_retry(sb, goal, state, &channel_id, thread_id).await?;
    println!(
        "[trace] run_define_goal_phase: active thread_id after initial post={}",
        thread_id
    );

    for _iteration in 0..MAX_DEFINE_GOAL_ITERATIONS {
        // Reviewer reads the thread and posts a question or refined-goal summary.
        match reviewer
            .analyze_define_goal_thread(sb, &thread_id)
            .await
        {
            Ok((done, post)) => {
                let post_type = if done { "summary" } else { "question" };
                let envelope = serde_json::json!({
                    "type": post_type,
                    "content": post,
                })
                .to_string();

                if let Ok(tid) = thread_id.parse::<Uuid>() {
                    let _ = sb
                        .post_to_thread(tid, "ferb-reviewer", &envelope)
                        .await
                        .map_err(|e| eprintln!("[warn] Failed to post reviewer response: {}", e));
                }

                println!("\n[ferb-reviewer]\n{}\n", post);

                if done {
                    if handle_summary_confirmation(sb, &thread_id, state, goal, &post).await? {
                        return Ok(());
                    }
                } else {
                    collect_and_post_answer(sb, &thread_id).await?;
                }
            }
            Err(e) => {
                eprintln!("[warn] Reviewer analyze_define_goal_thread failed: {}", e);
            }
        }
    }

    anyhow::bail!(
        "Define-goal phase exceeded {} iterations without user confirmation",
        MAX_DEFINE_GOAL_ITERATIONS
    )
}

async fn setup_define_goal_channel(
    sb: &SwitchboardClient,
    goal: &str,
    state: &mut FerbState,
) -> Option<(String, String)> {
    let channel = match sb.create_channel("ferb-general").await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[warn] Switchboard unavailable for define-goal: {}", e);
            return None;
        }
    };
    let channel_id_str = channel.id.to_string();
    println!("[trace] setup_define_goal_channel: channel_id={}", channel_id_str);

    let thread = match sb
        .create_thread(channel.id, &format!("Define Goal: {}", goal))
        .await
    {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[warn] Could not create define-goal thread: {}", e);
            return None;
        }
    };
    let thread_id_str = thread.id.to_string();
    println!("[trace] setup_define_goal_channel: thread_id={}", thread_id_str);

    state
        .channel_ids
        .insert("general".to_string(), channel_id_str.clone());
    state
        .thread_ids
        .insert("define-goal".to_string(), thread_id_str.clone());

    println!(
        "[trace] setup_define_goal_channel: stored channel_id={} thread_id={}",
        channel_id_str, thread_id_str
    );
    Some((channel_id_str, thread_id_str))
}

fn store_raw_goal(state: &mut FerbState, goal: &str) {
    state.set_artifact(
        "define-goal",
        serde_json::json!({ "original_task": goal, "refined_goal": goal }),
    );
    if let Some(t) = state.kanban_board.get_task_mut("define-goal") {
        t.status = TaskStatus::Done;
    }
}

/// Show a refined-goal summary and ask the user to confirm or reject it.
/// Returns true when the user confirms and the goal is stored.
async fn handle_summary_confirmation(
    sb: &SwitchboardClient,
    thread_id: &str,
    state: &mut FerbState,
    original_goal: &str,
    refined_content: &str,
) -> anyhow::Result<bool> {
    print!("Does this look right? (yes/no): ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    if input == "yes" || input == "y" {
        if let Ok(tid) = thread_id.parse::<Uuid>() {
            let _ = sb
                .post_to_thread(
                    tid,
                    "ferb-user-proxy",
                    &serde_json::json!({
                        "type": "confirmation",
                        "content": "Goal confirmed."
                    })
                    .to_string(),
                )
                .await;
        }

        state.set_artifact(
            "define-goal",
            serde_json::json!({
                "original_task": original_goal,
                "refined_goal": refined_content,
            }),
        );

        state.set_artifact(
            "confirmed-goal",
            serde_json::Value::String(refined_content.to_string()),
        );
        println!(
            "[trace] stored confirmed-goal artifact: {} chars",
            refined_content.len()
        );

        if let Some(t) = state.kanban_board.get_task_mut("define-goal") {
            t.status = TaskStatus::Done;
        }

        // Post a status update to the progress thread if it exists.
        if let Some(run_thread) = state.thread_ids.get("progress") {
            if let Ok(tid) = run_thread.parse::<Uuid>() {
                let _ = sb
                    .post_to_thread(
                        tid,
                        "system",
                        "Define Goal card complete — goal confirmed by user.",
                    )
                    .await;
            }
        }

        println!("\n✓ Goal confirmed.\n");
        return Ok(true);
    }

    // Rejection: collect feedback and post it so the reviewer can refine further.
    print!("What should be different? ");
    io::stdout().flush()?;
    let mut feedback = String::new();
    io::stdin().lock().read_line(&mut feedback)?;
    let feedback = feedback.trim();

    let reply = if feedback.is_empty() {
        "Please refine further.".to_string()
    } else {
        format!("Please refine further: {}", feedback)
    };

    if let Ok(tid) = thread_id.parse::<Uuid>() {
        let _ = sb
            .post_to_thread(
                tid,
                "ferb-user-proxy",
                &serde_json::json!({ "type": "status", "content": reply }).to_string(),
            )
            .await;
    }

    Ok(false)
}

/// Ask for the user's answer to a reviewer question and post it to the thread.
async fn collect_and_post_answer(
    sb: &SwitchboardClient,
    thread_id: &str,
) -> anyhow::Result<()> {
    print!("Your answer: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    let answer = answer.trim();

    if !answer.is_empty() {
        if let Ok(tid) = thread_id.parse::<Uuid>() {
            let _ = sb
                .post_to_thread(tid, "ferb-user-proxy", answer)
                .await
                .map_err(|e| eprintln!("[warn] Failed to post answer: {}", e));
        }
    }

    Ok(())
}

// ── Pipeline ──────────────────────────────────────────────────────────────

struct Agents<'a> {
    moderator: &'a Moderator,
    user_proxy: &'a UserProxy,
    reviewer: &'a Reviewer,
    worker: &'a Worker,
    approver: &'a Approver,
    sb: &'a SwitchboardClient,
    sb_run: &'a ferb_core::SwitchboardRunState,
}

fn print_completion_summary(state: &FerbState) {
    let done: Vec<_> = state
        .kanban_board
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .collect();
    let blocked: Vec<_> = state
        .kanban_board
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Blocked)
        .collect();
    let remaining: Vec<_> = state
        .kanban_board
        .tasks
        .iter()
        .filter(|t| t.status != TaskStatus::Done && t.status != TaskStatus::Blocked)
        .collect();

    if !done.is_empty() {
        println!("## Completed ({}):", done.len());
        for t in &done {
            println!("  ✓ {}", t.name);
        }
    }
    if !blocked.is_empty() {
        println!("\n## Blocked ({}):", blocked.len());
        for t in &blocked {
            println!("  ✗ {} (exceeded pass budget of {})", t.name, t.pass_budget);
        }
    }
    if !remaining.is_empty() {
        println!("\n## Remaining ({}):", remaining.len());
        for t in &remaining {
            println!("  ○ {} [{:?}]", t.name, t.status);
        }
    }
}

async fn run_pipeline(state: &mut FerbState, agents: &Agents<'_>) -> anyhow::Result<()> {
    let mut printed_headers: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut card_pass_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut prev_board_snapshot = String::new();

    for pass in 0..MAX_PASSES {
        state.pass = pass;
        println!("--- Pass {} ---", pass + 1);

        agents.moderator.reconcile(state);

        let got_input = agents.user_proxy.run_legacy(state)?;

        if got_input {
            agents.moderator.reconcile(state);
        }

        if state.kanban_board.all_done() {
            println!("\n=== All tasks complete ===\n");
            print_kanban(state);
            println!("## Final Artifacts\n");
            println!("{}", serde_json::to_string_pretty(&state.artifacts)?);
            return Ok(());
        }

        if state.kanban_board.all_complete() {
            println!("\n=== Workflow stopped: some cards blocked ===\n");
            print_kanban(state);
            print_completion_summary(state);
            return Ok(());
        }

        let task_ids: Vec<String> = state
            .kanban_board
            .tasks
            .iter()
            .map(|t| t.id.clone())
            .collect();

        for task_id in &task_ids {
            let (agent, task_name, task_status, pass_budget) =
                match state.kanban_board.get_task(task_id) {
                    Some(t) => (
                        t.agent.clone(),
                        t.name.clone(),
                        t.status.clone(),
                        t.pass_budget,
                    ),
                    None => continue,
                };

            // Skip tasks that are already done or blocked.
            if task_status == TaskStatus::Done || task_status == TaskStatus::Blocked {
                continue;
            }

            // Enforce per-card pass budget (0 = unlimited).
            *card_pass_counts.entry(task_id.clone()).or_insert(0) += 1;
            let passes_used = card_pass_counts[task_id.as_str()];
            if pass_budget > 0 && passes_used > pass_budget {
                eprintln!(
                    "[warn] Card '{}' exceeded pass budget ({} passes), marking Blocked",
                    task_name, pass_budget
                );
                if let Some(t) = state.kanban_board.get_task_mut(task_id) {
                    t.status = TaskStatus::Blocked;
                }
                continue;
            }

            // Print a section header the first time each task becomes active.
            if !printed_headers.contains(task_id) {
                println!("\n=== {} ===\n", task_name);
                printed_headers.insert(task_id.clone());
            }

            let status_before = state.kanban_board.get_task(task_id).map(|t| t.status.clone());

            match agent.as_str() {
                "ferb-reviewer" => {
                    agents.reviewer.run_legacy(state, task_id).await?;
                }
                "ferb-worker" => {
                    agents.worker.run_legacy(state, task_id).await?;
                }
                "ferb-approver" => {
                    agents.approver.run_legacy(state, task_id);
                }
                _ => {
                    eprintln!("[warn] Unknown agent: {}", agent);
                }
            }

            let status_after = state.kanban_board.get_task(task_id).map(|t| t.status.clone());

            if status_after != status_before {
                let final_status = status_after.unwrap_or(TaskStatus::Pending);
                switchboard_post_agent_completion(
                    agents.sb, agents.sb_run, &agent, task_id, &task_name, &final_status,
                )
                .await;
            }
        }

        print_kanban(state);

        // Cycle detection: if the board is identical to last pass, nothing can progress.
        let current_snapshot =
            serde_json::to_string(&state.kanban_board).unwrap_or_default();
        if !prev_board_snapshot.is_empty() && current_snapshot == prev_board_snapshot {
            eprintln!(
                "\nERROR: Infinite loop detected — board state unchanged after pass {}.",
                pass + 1
            );
            eprintln!("No agent made any progress. Check your workflow for missing inputs or unreachable approval conditions.\n");
            print_kanban(state);
            print_completion_summary(state);
            anyhow::bail!(
                "Cycle detected: board state was identical after passes {} and {}",
                pass,
                pass + 1
            );
        }
        prev_board_snapshot = current_snapshot;

        if pass == MAX_PASSES - 1 {
            println!(
                "\nMax passes ({}) reached without completing all tasks.\n",
                MAX_PASSES
            );
            print_kanban(state);
            print_completion_summary(state);
            return Ok(());
        }
    }

    Ok(())
}

pub async fn run_task(
    goal: &str,
    channel_id: Option<&str>,
    workflow_path: Option<&str>,
    config: &FerbConfig,
) -> anyhow::Result<()> {
    let sb_url = &config.switchboard.url;
    let tw_url = &config.tramway.url;
    let model = &config.tramway.model;

    let sb = SwitchboardClient::new(sb_url);
    let moderator = Moderator::new(sb_url);
    let user_proxy = UserProxy::new(sb_url);
    let reviewer = Reviewer::new(sb_url, tw_url, model);
    let worker = Worker::new(sb_url, tw_url, model);
    let approver = Approver::new(sb_url);

    let wf_raw = workflow_path
        .map(String::from)
        .or_else(|| std::env::var("FERB_WORKFLOW").ok())
        .unwrap_or_else(|| config.workflow.default.clone());
    let wf_path = expand_tilde(&wf_raw);
    let wf_content = std::fs::read_to_string(&wf_path)
        .map_err(|e| anyhow::anyhow!("Failed to load workflow {}: {}", wf_path.display(), e))?;

    // Card-based workflow (web-development.yaml style with `cards:` key).
    if is_card_workflow(&wf_content) {
        return run_card_workflow_task(goal, channel_id, &wf_content, config).await;
    }

    // Legacy task-based pipeline.
    let kanban_board = load_kanban_from_str(&wf_content)
        .map_err(|e| anyhow::anyhow!("Failed to parse workflow {}: {}", wf_path.display(), e))?;

    let mut state = FerbState::new(kanban_board);

    if let Ok(wf_val) = serde_yaml::from_str::<serde_json::Value>(&wf_content) {
        state.active_workflow = Some(wf_val);
    }

    // ── Switchboard health check ──────────────────────────────────────────
    sb.health_check().await.map_err(|e| {
        anyhow::anyhow!(
            "Error: {}\nRun 'ferb up' to start all required services.",
            e
        )
    })?;

    println!("\n=== Ferb ===\n");

    // ── Define Goal phase (conversation-based) ────────────────────────────
    if state.kanban_board.get_task("define-goal").is_some() {
        println!("=== Define Goal ===\n");
        run_define_goal_phase(&mut state, &sb, &reviewer, goal).await?;
    } else {
        // Legacy seed: no define-goal task, seed the goal via message channel.
        state.send_message("user", "ferb-reviewer", "define-goal", goal);
    }

    // ── Switchboard run tracking (progress channel) ───────────────────────
    // Reuse the channel created during define-goal phase if available.
    let define_goal_channel = state.channel_ids.get("general").cloned();
    let effective_channel = channel_id
        .map(String::from)
        .or(define_goal_channel);

    let sb_run = switchboard_start(&sb, goal, effective_channel.as_deref()).await?;

    // Store the progress thread in state so handle_summary_confirmation can reach it.
    state.channel_ids.insert("progress".to_string(), sb_run.channel_id.clone());
    state.thread_ids.insert("progress".to_string(), sb_run.thread_id.clone());

    let agents = Agents {
        moderator: &moderator,
        user_proxy: &user_proxy,
        reviewer: &reviewer,
        worker: &worker,
        approver: &approver,
        sb: &sb,
        sb_run: &sb_run,
    };

    let result = run_pipeline(&mut state, &agents).await;

    match &result {
        Ok(()) => switchboard_finish_success(&sb, &sb_run, &state).await,
        Err(e) => switchboard_finish_failure(&sb, &sb_run, &format!("{}", e)).await,
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_labeled_question() {
        let raw = r#"{"type":"question","content":"What is the target audience?"}"#;
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "question");
        assert_eq!(c, "What is the target audience?");
    }

    #[test]
    fn parse_labeled_summary() {
        let raw = r#"{"type":"summary","content":"Refined Goal: Build an app."}"#;
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "summary");
        assert!(c.contains("Refined Goal"));
    }

    #[test]
    fn parse_labeled_non_json_falls_back() {
        let raw = "some plain text";
        let (t, c) = parse_labeled_post_content(raw);
        assert_eq!(t, "status");
        assert_eq!(c, "some plain text");
    }

    #[test]
    fn parse_labeled_missing_type_defaults_to_status() {
        let raw = r#"{"content":"just a content field"}"#;
        let (t, _) = parse_labeled_post_content(raw);
        assert_eq!(t, "status");
    }

    #[test]
    fn store_raw_goal_marks_define_goal_done() {
        use ferb_core::{KanbanBoard, KanbanTask, TaskStatus};

        let board = KanbanBoard {
            tasks: vec![KanbanTask {
                id: "define-goal".to_string(),
                name: "Define Goal".to_string(),
                agent: "ferb-user-proxy".to_string(),
                prompt: None,
                status: TaskStatus::Pending,
                inputs: vec![],
                reviews: None,
                approves: None,
                max_iterations: 5,
                iterations_used: 0,
                questions: vec![],
                comments: vec![],
                success_criteria: vec![],
                pass_budget: 3,
            }],
        };
        let mut state = FerbState::new(board);
        store_raw_goal(&mut state, "Build a calculator");

        assert_eq!(
            state.kanban_board.get_task("define-goal").unwrap().status,
            TaskStatus::Done
        );
        let artifact = state.get_artifact("define-goal").unwrap();
        assert_eq!(artifact["original_task"], "Build a calculator");
    }
}
