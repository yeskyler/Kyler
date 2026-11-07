# Evaluations

`kyler-evals` measures agent functionality through GAIA and startup performance through
isolated probes. Evaluation runs are independent of installation and interactive
operation. Detailed benchmark settings and report semantics are documented in
`kyler-rs/crates/kyler-evals/README.md` in the source checkout.

## GAIA

Store authorized dataset downloads under the following directory:

```text
kyler-rs/crates/kyler-evals/datasets/GAIA/2023/
├── validation/
│   └── metadata.parquet
└── test/
    └── metadata.parquet
```

All attachments referenced by the dataset must be present. Legacy `metadata.jsonl` is
supported and takes precedence when present. A Git LFS pointer is not the dataset
content. Dataset files and local reports are excluded from version control.
Redistribution is subject to the dataset license and access restrictions.

From the repository root, validate the dataset without a model call:

```sh
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-evals -- gaia check
```

Evaluation execution uses the configured model and integrations and may incur provider
and tool charges:

```sh
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-evals -- gaia run --limit 5
```

The evaluator uses the real runtime with task-specific workspaces and stores attempt
evidence and scoring reports. Reference answers are not passed to the agent. Unavailable
test-split answers remain unscored. There is no built-in web search. MCP services and
user skills are included only when explicitly configured.

The default application configuration is `~/.kyler/config.toml`. `--app-config` selects
another application file; `--eval-config` selects evaluation settings. Default results
stay under `kyler-rs/crates/kyler-evals/results/`.

## Startup performance

Build release binaries before collecting startup measurements:

```sh
cargo build --manifest-path kyler-rs/Cargo.toml --release -p kyler-cli -p kyler-evals
kyler-rs/target/release/kyler-evals startup run --scenario process-cold --profile minimal --repeat 30
kyler-rs/target/release/kyler-evals startup run --scenario session-cold --mode mock --repeat 30
kyler-rs/target/release/kyler-evals startup run --scenario session-warm --mode mock --repeat 30
kyler-rs/target/release/kyler-evals startup run --scenario session-restore --restore-turns 100
```

Process-cold launches a new service and checks actual HTTP health. Session-cold,
session-warm, and session-restore measure different runtime/history paths. The harness
uses private temporary configuration and does not restart the installed service or
import normal user conversations.

Mock mode is the default for session measurements. The Default profile can connect to
configured external services. Minimal and synthetic profiles provide isolated local
measurements. Live mode uses the configured provider and can incur charges.

These probes do not clear operating-system caches. Report build profile, machine,
scenario, sample count, and enabled capabilities when comparing results. Baseline
comparison procedures, report fields, and measurement limits are defined in
`kyler-rs/crates/kyler-evals/README.md`.

See [development](development.md) for test commands and [architecture](architecture.md)
for the runtime being measured.
