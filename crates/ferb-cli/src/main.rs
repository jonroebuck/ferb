use ferb_approver::Approver;
use ferb_core::{FerbState, KanbanBoard, KanbanTask, TaskStatus, TramwayClient};
use ferb_moderator::Moderator;
use ferb_reviewer::Reviewer;
use ferb_user_proxy::UserProxy;
use ferb_worker::Worker;
use serde::Deserialize;

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

fn parse_input() -> anyhow::Result<String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        anyhow::bail!("Usage: ferb <goal text>");
    }
    Ok(args[1..].join(" "))
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let input = parse_input()?;

    let tramway_url =
        std::env::var("TRAMWAY_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client = TramwayClient::new(&tramway_url);

    let workflow_path =
        std::env::var("FERB_WORKFLOW").unwrap_or_else(|_| "workflows/default.yaml".to_string());
    let kanban_board = load_workflow(&workflow_path)?;

    let mut state = FerbState::new(kanban_board);

    state.send_message("user", "ferb-reviewer", "define-goal", &input);

    let moderator = Moderator;
    let user_proxy = UserProxy;
    let approver = Approver;
    let reviewer = Reviewer;
    let worker = Worker;

    println!("\n=== Ferb ===\n");

    for pass in 0..MAX_PASSES {
        state.pass = pass;
        println!("--- Pass {} ---", pass + 1);

        moderator.reconcile(&mut state);

        let got_input = user_proxy.run(&mut state)?;

        if got_input {
            moderator.reconcile(&mut state);
        }

        if state.kanban_board.all_done() {
            println!("\n=== All tasks complete ===\n");
            print_kanban(&state);

            println!("## Final Artifacts\n");
            println!("{}", serde_json::to_string_pretty(&state.artifacts)?);
            break;
        }

        let task_ids: Vec<String> = state
            .kanban_board
            .tasks
            .iter()
            .map(|t| t.id.clone())
            .collect();

        for task_id in &task_ids {
            let agent = state
                .kanban_board
                .get_task(task_id)
                .map(|t| t.agent.clone())
                .unwrap_or_default();

            match agent.as_str() {
                "ferb-reviewer" => {
                    reviewer.run(&mut state, task_id, &client).await?;
                }
                "ferb-worker" => {
                    worker.run(&mut state, task_id, &client).await?;
                }
                "ferb-approver" => {
                    approver.run(&mut state, task_id);
                }
                _ => {
                    eprintln!("[warn] Unknown agent: {}", agent);
                }
            }
        }

        print_kanban(&state);

        if pass == MAX_PASSES - 1 {
            anyhow::bail!(
                "Max passes ({}) reached without completing all tasks",
                MAX_PASSES
            );
        }
    }

    Ok(())
}
