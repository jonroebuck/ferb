use ferb_approver::Approver;
use ferb_core::{
    FerbState, KanbanBoard, KanbanTask, SwitchboardClient, SwitchboardRunState, TaskStatus,
    TramwayClient,
};
use ferb_moderator::Moderator;
use ferb_reviewer::Reviewer;
use ferb_user_proxy::UserProxy;
use ferb_worker::Worker;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

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
    workflow: String,
    tasks: Vec<WorkflowTaskDef>,
}

fn load_workflow(path: &str) -> anyhow::Result<KanbanBoard> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to load workflow {}: {}", path, e))?;
    let def: WorkflowDef = serde_yaml::from_str(&content)?;
    println!("Loaded workflow: {}", def.workflow);

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

struct CliArgs {
    goal: String,
    channel_id: Option<String>,
}

fn parse_args() -> anyhow::Result<CliArgs> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        anyhow::bail!("Usage: ferb <goal text> [--channel <id>]");
    }

    let mut goal_parts = Vec::new();
    let mut channel_id = None;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--channel" {
            if i + 1 >= args.len() {
                anyhow::bail!("--channel requires a value");
            }
            channel_id = Some(args[i + 1].clone());
            i += 2;
        } else {
            goal_parts.push(args[i].clone());
            i += 1;
        }
    }

    if goal_parts.is_empty() {
        anyhow::bail!("Usage: ferb <goal text> [--channel <id>]");
    }

    Ok(CliArgs {
        goal: goal_parts.join(" "),
        channel_id,
    })
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
    sb: &SwitchboardClient,
    title: &str,
    channel_id: Option<&str>,
) -> Option<SwitchboardRunState> {
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

    Some(SwitchboardRunState {
        issue_id: issue.id,
        channel_id: ch_id,
        thread_id,
    })
}

async fn switchboard_post_agent_completion(
    sb: &SwitchboardClient,
    run: &SwitchboardRunState,
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
    sb: &SwitchboardClient,
    run: &SwitchboardRunState,
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
        eprintln!("[warn] Switchboard: failed to post completion summary: {}", e);
    }

    if let Err(e) = sb.transition_issue(&run.issue_id, "done").await {
        eprintln!("[warn] Switchboard: failed to transition issue to done: {}", e);
    }
}

async fn switchboard_finish_failure(
    sb: &SwitchboardClient,
    run: &SwitchboardRunState,
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

async fn run_pipeline(
    state: &mut FerbState,
    client: &TramwayClient,
    sb: &SwitchboardClient,
    sb_run: &Option<SwitchboardRunState>,
) -> anyhow::Result<()> {
    let moderator = Moderator;
    let user_proxy = UserProxy;
    let approver = Approver;
    let reviewer = Reviewer;
    let worker = Worker;

    for pass in 0..MAX_PASSES {
        state.pass = pass;
        println!("--- Pass {} ---", pass + 1);

        moderator.reconcile(state);

        let got_input = user_proxy.run(state)?;

        if got_input {
            moderator.reconcile(state);
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
                    reviewer.run(state, task_id, client).await?;
                }
                "ferb-worker" => {
                    worker.run(state, task_id, client).await?;
                }
                "ferb-approver" => {
                    approver.run(state, task_id);
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
                if let Some(run) = sb_run {
                    let final_status = status_after.unwrap_or(TaskStatus::Pending);
                    switchboard_post_agent_completion(
                        sb, run, &agent, task_id, &task_name, &final_status,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;

    let tramway_url =
        std::env::var("TRAMWAY_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client = TramwayClient::new(&tramway_url);

    let switchboard_url =
        std::env::var("SWITCHBOARD_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let sb = SwitchboardClient::new(&switchboard_url);

    let workflow_path =
        std::env::var("FERB_WORKFLOW").unwrap_or_else(|_| "workflows/default.yaml".to_string());
    let kanban_board = load_workflow(&workflow_path)?;

    let mut state = FerbState::new(kanban_board);

    state.send_message("user", "ferb-reviewer", "define-goal", &args.goal);

    let sb_run = switchboard_start(&sb, &args.goal, args.channel_id.as_deref()).await;

    println!("\n=== Ferb ===\n");

    let result = run_pipeline(&mut state, &client, &sb, &sb_run).await;

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
