use ferb_approver::Approver;
use ferb_core::{
    FerbState, KanbanBoard, KanbanTask, TaskStatus,
};
use ferb_moderator::Moderator;
use ferb_reviewer::Reviewer;
use ferb_user_proxy::UserProxy;
use ferb_worker::Worker;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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

use crate::FerbConfig;

const MAX_PASSES: usize = 100;

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
}

#[derive(Debug, Deserialize)]
struct WorkflowDef {
    #[allow(dead_code)]
    workflow: String,
    tasks: Vec<WorkflowTaskDef>,
}

fn load_workflow(path: &str) -> anyhow::Result<KanbanBoard> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to load workflow {}: {}", path, e))?;
    let def: WorkflowDef = serde_yaml::from_str(&content)?;

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
        })
        .collect();

    Ok(KanbanBoard { tasks })
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
    sb: &ferb_core::SwitchboardClient,
    title: &str,
    channel_id: Option<&str>,
) -> Option<ferb_core::SwitchboardRunState> {
    let issue = match sb.create_issue(title, "backlog").await {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[warn] Switchboard: failed to create issue: {}", e);
            return None;
        }
    };

    let ch_id = if let Some(id) = channel_id {
        id.to_string()
    } else {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let slug = slugify(title);
        let name = format!("ferb-{}-{}", ts, slug);
        match sb.create_channel(&name).await {
            Ok(ch) => ch.id,
            Err(e) => {
                eprintln!("[warn] Switchboard: failed to create channel: {}", e);
                return None;
            }
        }
    };

    if let Err(e) = sb.transition_issue(&issue.id, "in_progress").await {
        eprintln!("[warn] Switchboard: failed to transition issue: {}", e);
    }

    let thread_id = match sb
        .create_thread(&ch_id, &format!("Ferb run started: {}", title))
        .await
    {
        Ok(t) => t.id,
        Err(e) => {
            eprintln!("[warn] Switchboard: failed to create thread: {}", e);
            return None;
        }
    };

    Some(ferb_core::SwitchboardRunState {
        issue_id: issue.id,
        channel_id: ch_id,
        thread_id,
    })
}

async fn switchboard_post_agent_completion(
    sb: &ferb_core::SwitchboardClient,
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
    if let Err(e) = sb
        .post_to_thread(&run.channel_id, &run.thread_id, &content)
        .await
    {
        eprintln!("[warn] Switchboard: failed to post agent completion: {}", e);
    }
}

async fn switchboard_finish_success(
    sb: &ferb_core::SwitchboardClient,
    run: &ferb_core::SwitchboardRunState,
    state: &FerbState,
) {
    let mut summary = String::from("Ferb run completed successfully.\n\nCompleted tasks:");
    for task in &state.kanban_board.tasks {
        summary.push_str(&format!("\n- {} [{:?}]", task.name, task.status));
    }

    if let Err(e) = sb
        .post_to_thread(&run.channel_id, &run.thread_id, &summary)
        .await
    {
        eprintln!(
            "[warn] Switchboard: failed to post completion summary: {}",
            e
        );
    }

    if let Err(e) = sb.transition_issue(&run.issue_id, "done").await {
        eprintln!(
            "[warn] Switchboard: failed to transition issue to done: {}",
            e
        );
    }
}

async fn switchboard_finish_failure(
    sb: &ferb_core::SwitchboardClient,
    run: &ferb_core::SwitchboardRunState,
    error: &str,
) {
    let content = format!("Ferb run failed: {}", error);
    if let Err(e) = sb
        .post_to_thread(&run.channel_id, &run.thread_id, &content)
        .await
    {
        eprintln!("[warn] Switchboard: failed to post error details: {}", e);
    }

    if let Err(e) = sb.transition_issue(&run.issue_id, "blocked").await {
        eprintln!(
            "[warn] Switchboard: failed to transition issue to blocked: {}",
            e
        );
    }
}

struct Agents<'a> {
    moderator: &'a Moderator,
    user_proxy: &'a UserProxy,
    reviewer: &'a Reviewer,
    worker: &'a Worker,
    approver: &'a Approver,
    sb: &'a ferb_core::SwitchboardClient,
    sb_run: &'a Option<ferb_core::SwitchboardRunState>,
}

async fn run_pipeline(state: &mut FerbState, agents: &Agents<'_>) -> anyhow::Result<()> {
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

        let task_ids: Vec<String> = state
            .kanban_board
            .tasks
            .iter()
            .map(|t| t.id.clone())
            .collect();

        for task_id in &task_ids {
            let (agent, task_name) = match state.kanban_board.get_task(task_id) {
                Some(t) => (t.agent.clone(), t.name.clone()),
                None => continue,
            };

            let status_before = state
                .kanban_board
                .get_task(task_id)
                .map(|t| t.status.clone());

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

            let status_after = state
                .kanban_board
                .get_task(task_id)
                .map(|t| t.status.clone());

            if status_after != status_before {
                if let Some(run) = agents.sb_run {
                    let final_status = status_after.unwrap_or(TaskStatus::Pending);
                    switchboard_post_agent_completion(
                        agents.sb, run, &agent, task_id, &task_name, &final_status,
                    )
                    .await;
                }
            }
        }

        print_kanban(state);

        if pass == MAX_PASSES - 1 {
            anyhow::bail!(
                "Max passes ({}) reached without completing all tasks",
                MAX_PASSES
            );
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

    let sb = ferb_core::SwitchboardClient::new(sb_url);
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
    let wf_path_str = wf_path.to_string_lossy();
    let kanban_board = load_workflow(&wf_path_str)?;

    let mut state = FerbState::new(kanban_board);
    state.send_message("user", "ferb-reviewer", "define-goal", goal);

    if let Ok(wf_content) = std::fs::read_to_string(&wf_path) {
        if let Ok(wf_val) = serde_yaml::from_str::<serde_json::Value>(&wf_content) {
            state.active_workflow = Some(wf_val);
        }
    }

    let sb_run = switchboard_start(&sb, goal, channel_id).await;

    println!("\n=== Ferb ===\n");

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
        Ok(()) => {
            if let Some(run) = &sb_run {
                switchboard_finish_success(&sb, run, &state).await;
            }
        }
        Err(e) => {
            if let Some(run) = &sb_run {
                switchboard_finish_failure(&sb, run, &format!("{}", e)).await;
            }
        }
    }

    result
}
