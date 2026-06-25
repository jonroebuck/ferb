use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const BUNDLED_COMPOSE: &str = include_str!("../docker-compose.yml");

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

fn write_secret(ferb_dir: &Path, name: &str, value: &str) -> anyhow::Result<()> {
    let secrets_dir = ferb_dir.join("secrets");
    std::fs::create_dir_all(&secrets_dir)?;
    std::fs::write(secrets_dir.join(name), value)?;
    Ok(())
}

fn load_secrets(ferb_dir: &Path) -> HashMap<String, String> {
    let secrets_dir = ferb_dir.join("secrets");
    let mut secrets = HashMap::new();
    for key in SECRET_KEYS {
        if let Ok(val) = std::env::var(key) {
            secrets.insert(key.to_string(), val);
        } else if let Ok(val) = std::fs::read_to_string(secrets_dir.join(key)) {
            let val = val.trim().to_string();
            if !val.is_empty() {
                secrets.insert(key.to_string(), val);
            }
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

    println!("Waiting for services to be ready...");

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

        if start.elapsed() > timeout {
            eprintln!("[warn] Timed out waiting for services — they may still be starting");
            return Ok(());
        }

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

fn cmd_up_interactive(ferb_dir: &Path) -> anyhow::Result<()> {
    check_docker()?;

    std::fs::create_dir_all(ferb_dir.join("secrets"))?;

    ensure_compose_file(ferb_dir)?;

    let anthropic_key = if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        println!("ANTHROPIC_API_KEY: (using environment variable)");
        key
    } else {
        let key = prompt("ANTHROPIC_API_KEY", "")?;
        if key.is_empty() {
            anyhow::bail!("ANTHROPIC_API_KEY is required");
        }
        key
    };

    let openai_key = if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        println!("OPENAI_API_KEY: (using environment variable)");
        key
    } else {
        prompt("OPENAI_API_KEY (optional, press Enter to skip)", "")?
    };

    let gemini_key = if let Ok(key) = std::env::var("GEMINI_API_KEY") {
        println!("GEMINI_API_KEY: (using environment variable)");
        key
    } else {
        prompt("GEMINI_API_KEY (optional, press Enter to skip)", "")?
    };

    let switchboard_url = prompt("Switchboard URL", "http://localhost:4080")?;
    let tramway_url = prompt("Tramway URL", "http://localhost:8080")?;

    let config = crate::FerbToml {
        server: crate::ServerToml { port: 9090 },
        switchboard: crate::UrlToml {
            url: switchboard_url.clone(),
        },
        tramway: crate::UrlToml {
            url: tramway_url,
        },
    };
    let toml_str = toml::to_string_pretty(&config)?;
    std::fs::write(ferb_dir.join("ferb.toml"), toml_str)?;

    write_secret(ferb_dir, "ANTHROPIC_API_KEY", &anthropic_key)?;
    if !openai_key.is_empty() {
        write_secret(ferb_dir, "OPENAI_API_KEY", &openai_key)?;
    }
    if !gemini_key.is_empty() {
        write_secret(ferb_dir, "GEMINI_API_KEY", &gemini_key)?;
    }

    docker_compose_up(ferb_dir)?;
    wait_for_services(ferb_dir)?;

    println!("\nFerb is ready!");
    println!("Switchboard: {}", switchboard_url);

    Ok(())
}

fn cmd_up_ci(ferb_dir: &Path) -> anyhow::Result<()> {
    check_docker()?;
    ensure_compose_file(ferb_dir)?;

    docker_compose_up(ferb_dir)?;
    wait_for_services(ferb_dir)?;

    let switchboard_url =
        std::env::var("SWITCHBOARD_URL").unwrap_or_else(|_| "http://localhost:4080".to_string());
    println!("\nFerb is ready!");
    println!("Switchboard: {}", switchboard_url);

    Ok(())
}

pub fn cmd_up() -> anyhow::Result<()> {
    let ferb_dir = crate::ferb_dir();
    if is_interactive() {
        cmd_up_interactive(&ferb_dir)
    } else {
        cmd_up_ci(&ferb_dir)
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
