# ferb

A generic loop engineering framework for artifact generation using a five-agent pipeline.

## Architecture

Ferb runs a structured pipeline of LLM-powered agents to produce verified artifacts:

```
Analyst → Planner → Test Planner → Maker ←→ Verifier (loop)
```

1. **Analyst** — Clarifies an ambiguous goal string into a structured `Goal` via iterative questioning
2. **Planner** — Converts a `Goal` into a `Plan` with ordered steps and verifiable success criteria
3. **Test Planner** — Produces a `TestSuite` of enumerated test cases from the `Plan`. Test cases are fixed once created
4. **Maker** — Generates an `Artifact` (text, HTML, JSON, Markdown) from the `Plan` and `TestSuite`
5. **Verifier** — Checks the `Artifact` against the `TestSuite`, returning pass/fail per test case with specific feedback

The Maker/Verifier loop runs until all tests pass or `max_iterations` is reached. On human approval, the Plan + TestSuite + Artifact are saved as an `ApprovedTemplate` for future reuse.

## Project Structure

```
ferb/
  Cargo.toml                  # workspace manifest
  crates/
    ferb-core/                # shared types, TramwayClient
    ferb-analyst/             # goal clarification agent
    ferb-planner/             # plan generation agent
    ferb-test-planner/        # test suite generation agent
    ferb-maker/               # artifact generation agent
    ferb-verifier/            # artifact verification agent
    ferb-cli/                 # CLI entry point, pipeline orchestration
  templates/
    store.json                # approved template store
```

## Usage

```sh
cargo run -p ferb-cli -- "your goal description here"
```

## Template Store

When an artifact passes verification and receives human approval, Ferb stores the Plan, TestSuite, and Artifact together in `templates/store.json` keyed by goal description. On subsequent runs, if a matching template exists, Ferb skips the Analyst/Planner/Test Planner stages and goes straight to the Maker with stored data.

## License

MIT
