mod run;
mod setup;

use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ferb", about = "Kanban-driven artifact generation")]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    run_args: RunArgs,
}

#[derive(Args)]
struct RunArgs {
    /// Goal text for the task runner
    goal: Vec<String>,

    /// Resume posting to an existing Switchboard channel
    #[arg(long)]
    channel: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// First-time setup wizard and start services
    Up,
    /// Start all services (docker compose up -d)
    Start,
    /// Stop all services (docker compose down)
    Stop,
    /// Show running containers and config
    Status,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FerbConfig {
    pub server: ServerConfig,
    pub switchboard: SwitchboardConfig,
    pub tramway: TramwayConfig,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ServerConfig {
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SwitchboardConfig {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TramwayConfig {
    pub url: String,
    pub model: String,
}

#[derive(Serialize)]
pub(crate) struct FerbToml {
    pub server: ServerToml,
    pub switchboard: SwitchboardToml,
    pub tramway: TramwayToml,
}

#[derive(Serialize)]
pub(crate) struct ServerToml {
    pub port: u16,
}

#[derive(Serialize)]
pub(crate) struct SwitchboardToml {
    pub url: String,
}

#[derive(Serialize)]
pub(crate) struct TramwayToml {
    pub url: String,
    pub model: String,
}

pub(crate) fn ferb_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ferb")
}

pub(crate) fn load_config() -> anyhow::Result<FerbConfig> {
    let cfg = config::Config::builder()
        .set_default("server.port", 9090)?
        .set_default("switchboard.url", "http://localhost:4080")?
        .set_default("tramway.url", "http://localhost:8080")?
        .set_default("tramway.model", "claude-sonnet-4-6")?
        .add_source(config::File::from(ferb_dir().join("ferb.toml")).required(false))
        .set_override_option("switchboard.url", std::env::var("SWITCHBOARD_URL").ok())?
        .set_override_option("tramway.url", std::env::var("TRAMWAY_URL").ok())?
        .set_override_option("tramway.model", std::env::var("FERB_MODEL").ok())?
        .build()?;

    Ok(cfg.try_deserialize()?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Up) => setup::cmd_up(),
        Some(Command::Start) => setup::cmd_start(),
        Some(Command::Stop) => setup::cmd_stop(),
        Some(Command::Status) => setup::cmd_status(),
        None => {
            if cli.run_args.goal.is_empty() {
                anyhow::bail!(
                    "Usage: ferb <goal text> [--channel <id>]\n       ferb up|start|stop|status"
                );
            }
            let goal = cli.run_args.goal.join(" ");
            let config = load_config()?;
            run::run_task(&goal, cli.run_args.channel.as_deref(), &config).await
        }
    }
}
