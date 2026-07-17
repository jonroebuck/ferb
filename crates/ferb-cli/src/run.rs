use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use ferb_agent_core::{CardContext, CardWorkflow, FerbAgent, RunState, SwitchboardClient, Uuid};
use ferb_core::FerbState;
use ferb_reviewer::Reviewer;
use ferb_worker::Worker;
use std::collections::HashMap;

use crate::FerbConfig;

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

// ── Card-based workflow pipeline ───────────────────────────────────────────

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
    let mut state = FerbState::new();
    state
        .channel_ids
        .insert("general".to_string(), run_state.channel_id.to_string());

    run_define_goal_phase(&mut state, sb, reviewer, goal).await?;

    // Post the confirmed goal to each output thread.
    if let Some(artifact) = state.get_artifact("define-goal") {
        run_state.confirmed_goal = Some(artifact.clone());
        for output_name in outputs {
            if let Some(&tid) = run_state.output_threads.get(output_name) {
                if let Err(e) = sb.post_to_thread(tid, "ferb-reviewer", &artifact).await {
                    eprintln!(
                        "[warn] Failed to post goal to '{}' thread: {}",
                        output_name, e
                    );
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
            run_state
                .output_threads
                .insert(output_name.clone(), thread.id);
        }

        // define-goal runs an interactive conversation; handled separately.
        if card.title == "define-goal" {
            run_define_goal_card(goal, run_state, sb, reviewer, &card.outputs).await?;
            continue;
        }

        // Build context string from all input threads.
        let input_context = build_input_context(sb, &card.inputs, &run_state.output_threads).await;

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

        let primary_agent = card
            .agents
            .first()
            .map(String::as_str)
            .unwrap_or("ferb-worker");
        let resp = match primary_agent {
            "ferb-worker" => worker.run(context, tramway).await?,
            "ferb-reviewer" => reviewer.run(context, tramway).await?,
            unknown => {
                eprintln!(
                    "[warn] Unknown agent '{}' for card '{}', skipping",
                    unknown, card.title
                );
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
                    eprintln!(
                        "[warn] Failed to post to output thread '{}': {}",
                        output_name, e
                    );
                }
            }
        }

        // Progress summary to the main thread.
        let summary = format!("Card '{}' completed.", card.title);
        if let Err(e) = sb
            .post_to_thread(run_state.thread_id, "system", &summary)
            .await
        {
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
    let reviewer = Reviewer::new(sb_url, tw_url, model).with_max_tokens(config.tramway.max_tokens);
    let worker = Worker::new(sb_url, tw_url, model).with_max_tokens(config.tramway.max_tokens);
    let tramway =
        ferb_core::TramwayClient::new(tw_url, model).with_max_tokens(config.tramway.max_tokens);

    sb.health_check().await.map_err(|e| {
        anyhow::anyhow!(
            "Error: {}\nRun 'ferb up' to start all required services.",
            e
        )
    })?;

    println!("\n=== Ferb ===\n");

    let issue = sb
        .create_issue(goal)
        .await
        .map_err(|e| anyhow::anyhow!("Switchboard setup failed — create_issue: {}", e))?;

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

    let result = run_card_pipeline(
        &workflow,
        goal,
        &mut run_state,
        &sb,
        &tramway,
        &worker,
        &reviewer,
    )
    .await;

    match &result {
        Ok(()) => {
            let _ = sb
                .post_to_thread(
                    run_state.thread_id,
                    "system",
                    "Workflow completed successfully.",
                )
                .await;
            if let Err(e) = sb.update_issue_status(run_state.issue_id, "done").await {
                eprintln!(
                    "[warn] Switchboard: failed to transition issue to done: {}",
                    e
                );
            }
        }
        Err(e) => {
            let msg = format!("Workflow failed: {}", e);
            let _ = sb.post_to_thread(run_state.thread_id, "system", &msg).await;
            if let Err(e2) = sb.update_issue_status(run_state.issue_id, "blocked").await {
                eprintln!(
                    "[warn] Switchboard: failed to transition issue to blocked: {}",
                    e2
                );
            }
        }
    }

    result
}

// ── Artifact filesystem write ──────────────────────────────────────────────

#[allow(dead_code)]
fn detect_extension(content: &str) -> &'static str {
    let t = content.trim_start();
    // Fast path: content starts directly with the artifact.
    if t.starts_with("<!DOCTYPE") || t.starts_with("<!doctype") || t.starts_with("<html") {
        return "html";
    }
    if t.starts_with('{') || t.starts_with('[') {
        return "json";
    }
    // Slow path: LLM may have emitted reasoning prose before the artifact.
    // Search the first 2000 chars for a definitive HTML marker before falling
    // back to the YAML heuristic (which matches any "key:\n" in prose too).
    let window = &t[..t.len().min(2000)];
    if window.contains("<!DOCTYPE ") || window.contains("<!doctype ") || window.contains("\n<html")
    {
        return "html";
    }
    if t.contains(":\n") || t.starts_with("---") {
        return "yaml";
    }
    "txt"
}

// Returns true if `line` looks like a YAML mapping key (e.g. "stores:" or "Any Store:").
#[allow(dead_code)]
fn is_yaml_key_line(line: &str) -> bool {
    let Some((key, _)) = line.split_once(':') else {
        return false;
    };
    let key = key.trim();
    !key.is_empty()
        && !key.contains('\n')
        && key.len() <= 64
        && key
            .chars()
            .all(|c| c.is_alphanumeric() || " _-".contains(c))
}

/// Strip any leading reasoning prose the LLM may have prepended before the
/// actual artifact content.  Returns a slice of the original string so no
/// allocation is needed.
fn strip_preamble<'a>(content: &'a str, ext: &str) -> &'a str {
    let t = content.trim_start();
    match ext {
        "html" => {
            if let Some(pos) = t.find("<!DOCTYPE ").or_else(|| t.find("<!doctype ")) {
                return &t[pos..];
            }
            // <html may appear inline; only strip if it starts a line.
            for (byte_pos, line) in iter_lines_with_pos(t) {
                if line.trim_start().starts_with("<html") {
                    return &t[byte_pos..];
                }
            }
            t
        }
        "json" => {
            if let Some(pos) = t.find('{').or_else(|| t.find('[')) {
                return &t[pos..];
            }
            t
        }
        "yaml" => {
            for (byte_pos, line) in iter_lines_with_pos(t) {
                let trimmed = line.trim_start();
                if trimmed == "---" || trimmed.starts_with("- ") || is_yaml_key_line(trimmed) {
                    return &t[byte_pos..];
                }
            }
            t
        }
        _ => t,
    }
}

#[allow(dead_code)]
fn iter_lines_with_pos(s: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut pos = 0;
    s.split('\n').map(move |line| {
        let start = pos;
        pos += line.len() + 1; // +1 for the '\n'
        (start, line)
    })
}

#[allow(dead_code)]
fn write_artifact_to_disk(
    state: &FerbState,
    artifact_id: &str,
    filename_stem: &str,
    output_dir: &std::path::Path,
) {
    let artifact = match state.get_artifact(artifact_id) {
        Some(v) => v,
        None => return,
    };
    let raw = artifact;
    if raw.trim().is_empty() {
        return;
    }
    let ext = detect_extension(&raw);
    let content = strip_preamble(&raw, ext);
    if content.trim().is_empty() {
        eprintln!(
            "[warn] Artifact '{}' is empty after stripping preamble; skipping write",
            artifact_id
        );
        return;
    }
    let path = output_dir.join(format!("{}.{}", filename_stem, ext));
    match std::fs::write(&path, content) {
        Ok(()) => println!("[info] Artifact written to {}", path.display()),
        Err(e) => eprintln!(
            "[warn] Failed to write artifact to {}: {}",
            path.display(),
            e
        ),
    }
}

#[allow(dead_code)]
fn write_artifacts_to_disk(state: &FerbState) {
    let output_dir = std::path::Path::new("ferb-output");
    if let Err(e) = std::fs::create_dir_all(output_dir) {
        eprintln!("[warn] Could not create output directory: {}", e);
        return;
    }
    write_artifact_to_disk(state, "make-artifact", "artifact", output_dir);
    write_artifact_to_disk(state, "make-data-file", "data-file", output_dir);
}

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
        if sb
            .post_to_thread(tid, "ferb-user-proxy", goal)
            .await
            .is_ok()
        {
            println!(
                "[trace] post_initial_goal_with_retry: initial post succeeded thread_id={}",
                thread_id
            );
            return Ok(thread_id);
        }
    }

    eprintln!("[warn] Initial post to define-goal thread failed, retrying with a fresh thread...");
    let ch_uuid: Uuid = channel_id.parse().map_err(|_| {
        anyhow::anyhow!(
            "Could not create define-goal thread on retry: invalid channel_id={}",
            channel_id
        )
    })?;
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

    println!(
        "[trace] post_initial_goal_with_retry: retry post succeeded thread_id={}",
        new_thread_id
    );
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
            store_raw_goal(state, goal)?;
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
    let thread_id = post_initial_goal_with_retry(sb, goal, state, &channel_id, thread_id).await?;
    println!(
        "[trace] run_define_goal_phase: active thread_id after initial post={}",
        thread_id
    );

    for _iteration in 0..MAX_DEFINE_GOAL_ITERATIONS {
        // Reviewer reads the thread and posts a question or refined-goal summary.
        match reviewer.analyze_define_goal_thread(sb, &thread_id).await {
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
                    if handle_summary_confirmation(sb, &thread_id, state, &post).await? {
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
    println!(
        "[trace] setup_define_goal_channel: channel_id={}",
        channel_id_str
    );

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
    println!(
        "[trace] setup_define_goal_channel: thread_id={}",
        thread_id_str
    );

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

fn store_raw_goal(state: &mut FerbState, goal: &str) -> anyhow::Result<()> {
    state.set_artifact("define-goal", None, goal)?;
    state.set_artifact("confirmed-goal", None, goal)?;
    Ok(())
}

/// Show a refined-goal summary and ask the user to confirm or reject it.
/// Returns true when the user confirms and the goal is stored.
async fn handle_summary_confirmation(
    sb: &SwitchboardClient,
    thread_id: &str,
    state: &mut FerbState,
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

        state.set_artifact("define-goal", None, refined_content)?;

        state.set_artifact("confirmed-goal", None, refined_content)?;
        println!(
            "[trace] stored confirmed-goal artifact: {} chars",
            refined_content.len()
        );

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
async fn collect_and_post_answer(sb: &SwitchboardClient, thread_id: &str) -> anyhow::Result<()> {
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

pub async fn run_task(
    goal: &str,
    channel_id: Option<&str>,
    workflow_path: Option<&str>,
    config: &FerbConfig,
) -> anyhow::Result<()> {
    let wf_raw = workflow_path
        .map(String::from)
        .or_else(|| std::env::var("FERB_WORKFLOW").ok())
        .unwrap_or_else(|| config.workflow.default.clone());
    let wf_path = expand_tilde(&wf_raw);
    let wf_content = std::fs::read_to_string(&wf_path)
        .map_err(|e| anyhow::anyhow!("Failed to load workflow {}: {}", wf_path.display(), e))?;

    run_card_workflow_task(goal, channel_id, &wf_content, config).await
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
    fn store_raw_goal_writes_define_and_confirmed_artifacts() {
        let mut state = FerbState::new();
        store_raw_goal(&mut state, "Build a calculator").unwrap();

        assert_eq!(
            state.get_artifact("define-goal").unwrap(),
            "Build a calculator"
        );
        assert_eq!(
            state.get_artifact("confirmed-goal").unwrap(),
            "Build a calculator"
        );
    }
}
