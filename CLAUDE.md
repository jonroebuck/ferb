# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Ferb is a kanban-driven artifact generation framework. A YAML workflow defines tasks on a kanban board. Three agent types (reviewer, worker, approver) process tasks in a loop, coordinated by a moderator and user proxy. The pipeline flows through four phases: Goal → Plan → Tests → Artifact, each with worker/reviewer/approver steps.

## Build Commands

```bash
cargo build                        # compile all crates
cargo test                         # run all tests
cargo test -p ferb-core            # run tests for a single crate
cargo clippy                       # lint
cargo fmt                          # format
cargo run -p ferb-cli -- "goal"    # run the pipeline
cargo run -p ferb-cli -- up        # first-time setup wizard
cargo run -p ferb-cli -- start     # start all Docker services
cargo run -p ferb-cli -- stop      # stop all Docker services
cargo run -p ferb-cli -- status    # show running containers and config
```

## Architecture

Cargo workspace with 8 crates under `crates/`:

- **ferb-core** — Shared types (FerbState, KanbanBoard, KanbanTask, ChannelMessage, etc.), TramwayClient (reqwest-based, talks to OpenAI-compatible API), and SwitchboardClient (issue tracking + channel messaging). Model set via `FERB_MODEL` env var, defaults to `claude/claude-sonnet-4-6`.
- **ferb-utils** — JSON parsing helpers: `clean_json()` strips markdown fences, `parse_json<T>()` cleans + sanitizes + deserializes with descriptive errors.
- **ferb-moderator** — Reconciles the message channel against the kanban board. Extracts questions from agent messages, matches user replies to unanswered questions.
- **ferb-user-proxy** — Handles stdin/stdout interaction. Prints messages directed to "user" and collects responses.
- **ferb-approver** — Gate agent. Marks a target task as Done when all its reviewers are Done and the target is ReadyForReview.
- **ferb-reviewer** — LLM-powered review agent. Loads a prompt file, builds context from kanban state + artifacts, calls Tramway, updates kanban status and message channel.
- **ferb-worker** — LLM-powered production agent. One-shot artifact generation. Loads prompt, calls Tramway, stores artifacts, sets task to ReadyForReview.
- **ferb-cli** — Entry point. Uses clap subcommands: `up` (setup wizard), `start`/`stop`/`status` (Docker service management), or bare `<goal>` (task runner). Loads config from `~/.ferb/ferb.toml` via the `config` crate with env var overrides. Integrates with Switchboard for issue tracking and channel messaging (best-effort). Supports `--channel <id>` to resume posting to an existing channel.

## Key Concepts

- **FerbState** — Central shared state: message channel, kanban board, artifacts (keyed by task id), pass counter.
- **KanbanBoard** — List of KanbanTasks with status tracking, input dependencies, questions, comments, success criteria.
- **Workflow YAML** — Defines the task graph in `workflows/`. Each task specifies its agent type, prompt file, inputs (dependencies), reviews/approves relationships, and max iterations.
- **Message Channel** — Agents communicate via ChannelMessage structs. The moderator reconciles these into kanban questions/answers.
- **Prompts** — System prompts loaded at runtime from `FERB_PROMPTS_DIR` (defaults to `./prompts`).

## Environment Variables

- `TRAMWAY_URL` — LLM API base URL (default: `http://localhost:8080`). Overrides `~/.ferb/ferb.toml`.
- `SWITCHBOARD_URL` — Switchboard API base URL for issue tracking and messaging (default: `http://localhost:4080`). Overrides `~/.ferb/ferb.toml`.
- `FERB_MODEL` — Model name for Tramway requests (default: `claude/claude-sonnet-4-6`)
- `FERB_PROMPTS_DIR` — Directory containing `.md` prompt files (default: `./prompts`)
- `FERB_WORKFLOW` — Path to workflow YAML file (default: `workflows/default.yaml`)
