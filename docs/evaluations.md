# Evaluations

`kyler-evals` measures agent functionality through GAIA and startup performance
through isolated probes. It is separate from normal installation and interactive
use. Detailed benchmark settings and report semantics are documented in
`kyler-rs/crates/kyler-evals/README.md` in the source checkout.

## GAIA

Place authorized dataset downloads in:

```text
kyler-rs/crates/kyler-evals/datasets/GAIA/2023/
├── validation/
│   └── metadata.parquet
└── test/
    └── metadata.parquet
```

Include referenced attachments. Legacy `metadata.jsonl` is supported and takes
precedence when present. A Git LFS pointer is not the dataset content. Dataset
files and local reports are ignored by Git and should not be redistributed.

From the repository root, validate the dataset without a model call:

```sh
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-evals -- gaia check
```

An actual evaluation uses configured models and may incur provider/tool charges:

```sh
cargo run --manifest-path kyler-rs/Cargo.toml -p kyler-evals -- gaia run --limit 5
```

The evaluator uses the real runtime with task-specific workspaces and stores
attempt evidence and scoring reports. Reference answers are not passed to the
agent. Test-split answers may be unavailable; the evaluator does not invent them.
There is no built-in web search. Explicitly configured MCP services and user
skills are optional dependencies, not assumed capabilities.

The default application configuration is `~/.kyler/config.toml`.
`--app-config` selects another application file; `--eval-config` selects evaluation
settings. Default results stay under `kyler-rs/crates/kyler-evals/results/`.

## Startup performance

Use release binaries before comparing timings:

```sh
cargo build --manifest-path kyler-rs/Cargo.toml --release -p kyler-cli -p kyler-evals
kyler-rs/target/release/kyler-evals startup run --scenario process-cold --profile minimal --repeat 30
kyler-rs/target/release/kyler-evals startup run --scenario session-cold --mode mock --repeat 30
kyler-rs/target/release/kyler-evals startup run --scenario session-warm --mode mock --repeat 30
kyler-rs/target/release/kyler-evals startup run --scenario session-restore --restore-turns 100
```

Process-cold launches a new service and checks actual HTTP health. Session-cold,
session-warm, and session-restore measure different runtime/history paths.
The harness uses private temporary configuration and does not restart the installed
service or import normal user conversations.

Mock mode is the default for session measurements. A Default profile can still
connect configured external services; use Minimal or synthetic profiles for local
comparisons. Live mode uses the configured provider and can incur charges.

These probes do not clear operating-system caches. Report build profile, machine,
scenario, sample count, and enabled capabilities when comparing results. Use the
benchmark's detailed README for baseline comparisons, report fields, and limits.

See [development](development.md) for test commands and
[architecture](architecture.md) for the runtime being measured.
