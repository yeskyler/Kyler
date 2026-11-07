use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::{Parser, Subcommand};

use kyler_evals::{
    config::EvalConfig,
    gaia, report,
    startup::{self, ModelMode, Profile, Scenario, Settings},
};

#[derive(Parser)]
#[command(
    name = "kyler-evals",
    version,
    about = "GAIA evaluations and Agent startup measurements"
)]
struct Args {
    /// Kyler application configuration; defaults to ~/.kyler/config.toml.
    #[arg(long, global = true)]
    app_config: Option<PathBuf>,
    /// GAIA evaluation settings (paths are relative to this file).
    #[arg(long, global = true)]
    eval_config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Gaia {
        #[command(subcommand)]
        command: Gaia,
    },
    Startup {
        #[command(subcommand)]
        command: Startup,
    },
    Report {
        run: PathBuf,
    },
    Compare {
        baseline: PathBuf,
        candidate: PathBuf,
    },
    #[command(name = "__probe", hide = true)]
    Probe {
        #[arg(long, value_enum)]
        scenario: Scenario,
        #[arg(long, value_enum)]
        mode: ModelMode,
        #[arg(long)]
        private: PathBuf,
        #[arg(long)]
        startup_settings: Option<PathBuf>,
        #[arg(long)]
        public_configuration: Option<PathBuf>,
    },
}
#[derive(Subcommand)]
enum Gaia {
    Check {
        #[arg(long)]
        level: Option<u8>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        split: Option<String>,
    },
    Run {
        #[arg(long)]
        level: Option<u8>,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        split: Option<String>,
    },
    Resume {
        run: PathBuf,
        /// Add a new attempt for unsuccessful tasks; never overwrite their evidence.
        #[arg(long)]
        retry_failed: bool,
    },
}
#[derive(Subcommand)]
enum Startup {
    Run {
        #[arg(long)]
        settings: Option<PathBuf>,
        #[arg(long, value_enum)]
        scenario: Option<Scenario>,
        #[arg(long, value_enum)]
        mode: Option<ModelMode>,
        #[arg(long)]
        repeat: Option<usize>,
        #[arg(long)]
        warmup: Option<usize>,
        #[arg(long)]
        timeout_seconds: Option<u64>,
        #[arg(long, value_enum)]
        profile: Option<Profile>,
        #[arg(long)]
        kyler_bin: Option<PathBuf>,
        #[arg(long)]
        baseline_bin: Option<PathBuf>,
        #[arg(long)]
        skill_count: Option<usize>,
        #[arg(long)]
        skill_instruction_bytes: Option<usize>,
        #[arg(long)]
        mcp_count: Option<usize>,
        #[arg(long)]
        mcp_delay_ms: Option<u64>,
        #[arg(long)]
        required_mcp: Vec<String>,
        #[arg(long)]
        restore_turns: Option<usize>,
        #[arg(long)]
        without_snapshot: bool,
    },
}
fn resolve_run(path: &Path, eval: &EvalConfig) -> PathBuf {
    if path.exists() {
        path.to_owned()
    } else {
        eval.run.results_directory.join(path)
    }
}
fn main() -> std::process::ExitCode {
    let worker = std::env::var_os("KYLER_INTERNAL_TOOL_WORKER").as_deref() == Some(OsStr::new("1"));
    if !worker && kyler_protocol::startup::enabled() {
        kyler_protocol::startup::emit(
            "main_entered",
            serde_json::json!({"build_profile":if cfg!(debug_assertions){"debug"}else{"release"},"version":env!("CARGO_PKG_VERSION")}),
        );
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Runtime failed: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if !worker && kyler_protocol::startup::enabled() {
        kyler_protocol::startup::mark("async_runtime_ready");
    }
    let status = runtime.block_on(execute());
    if !worker && kyler_protocol::startup::enabled() {
        kyler_protocol::startup::emit("process_finished", kyler_protocol::startup::resources());
    }
    status
}
async fn execute() -> std::process::ExitCode {
    if std::env::var_os("KYLER_INTERNAL_TOOL_WORKER").as_deref() == Some(OsStr::new("1")) {
        return match kyler_core::sandbox::worker::run_stdio().await {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(_) => std::process::ExitCode::FAILURE,
        };
    }
    match run(Args::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
async fn run(args: Args) -> Result<()> {
    let worker = std::env::current_exe()?;
    if let Command::Probe {
        scenario,
        mode,
        private,
        startup_settings,
        public_configuration,
    } = &args.command
    {
        let settings = if let Some(path) = startup_settings {
            serde_json::from_slice::<Settings>(&std::fs::read(path)?)?
        } else {
            Settings {
                scenario: *scenario,
                model_mode: *mode,
                ..Default::default()
            }
        };
        settings.validate()?;
        if let Err(error) = startup::probe::run(
            &settings,
            private,
            args.app_config.as_deref(),
            args.eval_config.as_deref(),
            &worker,
            public_configuration.as_deref(),
        )
        .await
        {
            kyler_protocol::startup::emit(
                "error",
                serde_json::json!({"message":error.to_string()}),
            );
            return Err(error);
        }
        return Ok(());
    }
    let mut eval = EvalConfig::load(args.eval_config.as_deref())?;
    match args.command {
        Command::Gaia { command } => match command {
            Gaia::Check {
                level,
                limit,
                split,
            } => {
                if let Some(split) = split {
                    eval.dataset.split = split;
                }
                eval.validate()?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&gaia::runner::check(&eval, level, limit)?)?
                );
            }
            Gaia::Run {
                level,
                limit,
                split,
            } => {
                if let Some(split) = split {
                    eval.dataset.split = split;
                }
                eval.validate()?;
                // Fail fast for missing/LFS data before loading credentials or starting tools.
                gaia::runner::check(&eval, level, limit)?;
                let app = kyler_evals::runtime::load_app(args.app_config.as_deref())?;
                gaia::runner::run(eval, app, level, limit, worker).await?;
            }
            Gaia::Resume { run, retry_failed } => {
                let directory = resolve_run(&run, &eval);
                let app = kyler_evals::runtime::load_app(args.app_config.as_deref())?;
                gaia::runner::resume(directory, app, worker, retry_failed).await?;
            }
        },
        Command::Startup {
            command:
                Startup::Run {
                    settings,
                    scenario,
                    mode,
                    repeat,
                    warmup,
                    timeout_seconds,
                    profile,
                    kyler_bin,
                    baseline_bin,
                    skill_count,
                    skill_instruction_bytes,
                    mcp_count,
                    mcp_delay_ms,
                    required_mcp,
                    restore_turns,
                    without_snapshot,
                },
        } => {
            let mut settings = Settings::load(settings.as_deref())?;
            if let Some(v) = scenario {
                settings.scenario = v;
            }
            if let Some(v) = mode {
                settings.model_mode = v;
            }
            if let Some(v) = repeat {
                settings.repeat = v;
            }
            if let Some(v) = warmup {
                settings.warmup = v;
            }
            if let Some(v) = timeout_seconds {
                settings.timeout_seconds = v;
            }
            if let Some(v) = profile {
                settings.profile = v;
            }
            if let Some(v) = kyler_bin {
                settings.kyler_bin = Some(v);
            }
            if let Some(v) = baseline_bin {
                settings.baseline_bin = Some(v);
            }
            if let Some(v) = skill_count {
                settings.skill_count = v;
            }
            if let Some(v) = skill_instruction_bytes {
                settings.skill_instruction_bytes = v;
            }
            if let Some(v) = mcp_count {
                settings.mcp_count = v;
            }
            if let Some(v) = mcp_delay_ms {
                settings.mcp_delay_ms = v;
            }
            if !required_mcp.is_empty() {
                settings.required_mcp = required_mcp;
            }
            if let Some(v) = restore_turns {
                settings.restore_turns = v;
            }
            if without_snapshot {
                settings.restore_snapshot = false;
            }
            startup::run(
                settings,
                args.app_config,
                eval.run.results_directory,
                args.eval_config,
                worker,
            )
            .await?;
        }
        Command::Report { run } => {
            let directory = resolve_run(&run, &eval);
            let manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(directory.join("manifest.json"))?)?;
            let value = if manifest["kind"] == "gaia" {
                let _lock = report::RunLock::acquire(&directory)?;
                report::gaia_summary(
                    &directory,
                    manifest["task_ids"]
                        .as_array()
                        .ok_or_else(|| anyhow::anyhow!("Missing task selection"))?
                        .len(),
                )?
            } else {
                serde_json::from_slice(&std::fs::read(directory.join("summary.json"))?)?
            };
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Command::Compare {
            baseline,
            candidate,
        } => {
            let result = report::compare(
                &resolve_run(&baseline, &eval),
                &resolve_run(&candidate, &eval),
            )?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Probe { .. } => unreachable!(),
    }
    Ok(())
}
