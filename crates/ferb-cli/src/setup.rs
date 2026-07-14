use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const BUNDLED_COMPOSE: &str = include_str!("../docker-compose.yml");
const BUNDLED_DEFAULT_WORKFLOW: &str = include_str!("../../../workflows/default.yaml");
const BUNDLED_WEB_DEV_WORKFLOW: &str = include_str!("../../../workflows/web-development.yaml");

const SECRET_KEYS: &[&str] = &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"];

fn is_interactive() -> bool {
    let ci = std::env::var("CI")
        .map(|v| v == "true")
        .unwrap_or(false);
    io::stdin().is_terminal() && !ci
}

fn check_docker() -> anyhow::Result<()> {
    let result = Command::new("docker").arg("info").output();
    match result {
        Ok(output) if output.status.success() => Ok(()),
        _ => {
            anyhow::bail!(
                "Docker is not running.\n\
                 Please install and start Docker Desktop: https://docs.docker.com/get-docker/"
            );
        }
    }
}

fn prompt(label: &str, default: &str) -> anyhow::Result<String> {
    if default.is_empty() {
        print!("{}: ", label);
    } else {
        print!("{} [{}]: ", label, default);
    }
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(input.to_string())
    }
}

fn secret_filename(env_name: &str) -> String {
    env_name.to_lowercase()
}

fn write_secret(ferb_dir: &Path, env_name: &str, value: &str) -> anyhow::Result<()> {
    let secrets_dir = ferb_dir.join("secrets");
    std::fs::create_dir_all(&secrets_dir)?;
    std::fs::write(secrets_dir.join(secret_filename(env_name)), value)?;
    Ok(())
}

fn read_secret(ferb_dir: &Path, env_name: &str) -> Option<String> {
    if let Ok(val) = std::env::var(env_name) {
        if !val.is_empty() {
            return Some(val);
        }
    }
    let path = ferb_dir.join("secrets").join(secret_filename(env_name));
    if let Ok(val) = std::fs::read_to_string(path) {
        let val = val.trim().to_string();
        if !val.is_empty() {
            return Some(val);
        }
    }
    None
}

fn load_secrets(ferb_dir: &Path) -> HashMap<String, String> {
    let mut secrets = HashMap::new();
    for key in SECRET_KEYS {
        if let Some(val) = read_secret(ferb_dir, key) {
            secrets.insert(key.to_string(), val);
        }
    }
    secrets
}

fn compose_cmd(ferb_dir: &Path) -> Command {
    let mut cmd = Command::new("docker");
    cmd.args([
        "compose",
        "-f",
        &ferb_dir.join("docker-compose.yml").to_string_lossy(),
    ]);
    for (key, val) in load_secrets(ferb_dir) {
        cmd.env(key, val);
    }
    cmd
}

fn docker_compose_pull(ferb_dir: &Path) -> anyhow::Result<()> {
    let status = compose_cmd(ferb_dir).arg("pull").status()?;
    if !status.success() {
        anyhow::bail!("docker compose pull failed");
    }
    Ok(())
}

fn docker_compose_up(ferb_dir: &Path) -> anyhow::Result<()> {
    let status = compose_cmd(ferb_dir).args(["up", "-d"]).status()?;
    if !status.success() {
        anyhow::bail!("docker compose up failed");
    }
    Ok(())
}

fn wait_for_services(ferb_dir: &Path) -> anyhow::Result<()> {
    let start = Instant::now();
    let timeout = Duration::from_secs(60);

    loop {
        let output = compose_cmd(ferb_dir)
            .args(["ps", "--format", "json"])
            .output()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
            if !lines.is_empty() {
                let all_running = lines.iter().all(|line| {
                    serde_json::from_str::<serde_json::Value>(line)
                        .map(|v| v["State"].as_str() == Some("running"))
                        .unwrap_or(false)
                });
                if all_running {
                    return Ok(());
                }
            }
        }

        let elapsed = start.elapsed().as_secs();
        if elapsed >= timeout.as_secs() {
            eprintln!("[warn] Timed out waiting for services — they may still be starting");
            return Ok(());
        }

        println!("[info] Waiting for services... ({}s)", elapsed);
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn ensure_compose_file(ferb_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(ferb_dir)?;
    let compose_path = ferb_dir.join("docker-compose.yml");
    if !compose_path.exists() {
        std::fs::write(&compose_path, BUNDLED_COMPOSE)?;
    }
    Ok(())
}

fn ensure_workflows(ferb_dir: &Path) -> anyhow::Result<()> {
    let wf_dir = ferb_dir.join("workflows");
    std::fs::create_dir_all(&wf_dir)?;
    std::fs::write(wf_dir.join("default.yaml"), BUNDLED_DEFAULT_WORKFLOW)?;
    std::fs::write(wf_dir.join("web-development.yaml"), BUNDLED_WEB_DEV_WORKFLOW)?;
    Ok(())
}

fn ensure_secret(
    ferb_dir: &Path,
    env_name: &str,
    required: bool,
) -> anyhow::Result<()> {
    if read_secret(ferb_dir, env_name).is_some() {
        println!("{}: (already set)", env_name);
        return Ok(());
    }

    let label = if required {
        env_name.to_string()
    } else {
        format!("{} (optional, press Enter to skip)", env_name)
    };
    let value = prompt(&label, "")?;

    if value.is_empty() && required {
        anyhow::bail!("{} is required", env_name);
    }
    if !value.is_empty() {
        write_secret(ferb_dir, env_name, &value)?;
    }
    Ok(())
}

fn cmd_up_interactive(ferb_dir: &Path, no_pull: bool) -> anyhow::Result<()> {
    check_docker()?;

    std::fs::create_dir_all(ferb_dir.join("secrets"))?;

    ensure_compose_file(ferb_dir)?;
    ensure_workflows(ferb_dir)?;

    // Config: only write on first run
    let toml_path = ferb_dir.join("ferb.toml");
    if !toml_path.exists() {
        let switchboard_url = prompt("Switchboard URL", "http://localhost:4080")?;
        let tramway_url = prompt("Tramway URL", "http://localhost:8080")?;

        let default_wf = ferb_dir
            .join("workflows")
            .join("default.yaml")
            .to_string_lossy()
            .to_string();
        let config = crate::FerbToml {
            server: crate::ServerToml { port: 9090 },
            switchboard: crate::SwitchboardToml {
                url: switchboard_url,
            },
            tramway: crate::TramwayToml {
                url: tramway_url,
                model: "claude/claude-sonnet-4-6".to_string(),
                max_tokens: 16384,
            },
            workflow: crate::WorkflowToml { default: default_wf },
        };
        let toml_str = toml::to_string_pretty(&config)?;
        std::fs::write(&toml_path, toml_str)?;
    } else {
        println!("Config already exists at {}", toml_path.display());
    }

    // Secrets: always check, prompt only for missing ones
    ensure_secret(ferb_dir, "ANTHROPIC_API_KEY", true)?;
    ensure_secret(ferb_dir, "OPENAI_API_KEY", false)?;
    ensure_secret(ferb_dir, "GEMINI_API_KEY", false)?;

    if !no_pull {
        println!("[info] Pulling latest images...");
        docker_compose_pull(ferb_dir)?;
    }
    println!("[info] Starting services...");
    docker_compose_up(ferb_dir)?;
    println!("[info] Waiting for services to be healthy...");
    wait_for_services(ferb_dir)?;

    let config = crate::load_config()?;
    println!("\nFerb is ready!");
    println!("Switchboard: {}", config.switchboard.url);

    Ok(())
}

fn cmd_up_ci(ferb_dir: &Path, no_pull: bool) -> anyhow::Result<()> {
    check_docker()?;
    ensure_compose_file(ferb_dir)?;
    ensure_workflows(ferb_dir)?;

    if !no_pull {
        println!("[info] Pulling latest images...");
        docker_compose_pull(ferb_dir)?;
    }
    println!("[info] Starting services...");
    docker_compose_up(ferb_dir)?;
    println!("[info] Waiting for services to be healthy...");
    wait_for_services(ferb_dir)?;

    let switchboard_url =
        std::env::var("SWITCHBOARD_URL").unwrap_or_else(|_| "http://localhost:4080".to_string());
    println!("\nFerb is ready!");
    println!("Switchboard: {}", switchboard_url);

    Ok(())
}

pub fn cmd_up(no_pull: bool) -> anyhow::Result<()> {
    let ferb_dir = crate::ferb_dir();
    if is_interactive() {
        cmd_up_interactive(&ferb_dir, no_pull)
    } else {
        cmd_up_ci(&ferb_dir, no_pull)
    }
}

pub fn cmd_start() -> anyhow::Result<()> {
    let ferb_dir = crate::ferb_dir();
    let compose_path = ferb_dir.join("docker-compose.yml");
    if !compose_path.exists() {
        anyhow::bail!("Ferb is not set up. Run 'ferb up' first.");
    }
    docker_compose_up(&ferb_dir)?;
    println!("Services started.");
    Ok(())
}

pub fn cmd_stop() -> anyhow::Result<()> {
    let ferb_dir = crate::ferb_dir();
    let compose_path = ferb_dir.join("docker-compose.yml");
    if !compose_path.exists() {
        anyhow::bail!("Ferb is not set up. Run 'ferb up' first.");
    }
    let status = compose_cmd(&ferb_dir).arg("down").status()?;
    if !status.success() {
        anyhow::bail!("docker compose down failed");
    }
    println!("Services stopped.");
    Ok(())
}

pub fn cmd_status() -> anyhow::Result<()> {
    let ferb_dir = crate::ferb_dir();

    let config = crate::load_config()?;
    println!("Configuration:");
    println!("  Server port:     {}", config.server.port);
    println!("  Switchboard URL: {}", config.switchboard.url);
    println!("  Tramway URL:     {}", config.tramway.url);
    println!();

    let compose_path = ferb_dir.join("docker-compose.yml");
    if !compose_path.exists() {
        println!("Services: not set up (run 'ferb up')");
        return Ok(());
    }

    println!("Services:");
    let _ = compose_cmd(&ferb_dir).arg("ps").status();

    Ok(())
}
