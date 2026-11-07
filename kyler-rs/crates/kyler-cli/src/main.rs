use std::{ffi::OsStr, io::Write, process::ExitCode};

use clap::Parser;

use kyler_cli::{args::Args, execute};

fn main() -> ExitCode {
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
            return ExitCode::FAILURE;
        }
    };
    if !worker && kyler_protocol::startup::enabled() {
        kyler_protocol::startup::mark("async_runtime_ready");
    }
    let status = runtime.block_on(run());
    if !worker && kyler_protocol::startup::enabled() {
        kyler_protocol::startup::emit("process_finished", kyler_protocol::startup::resources());
    }
    status
}

async fn run() -> ExitCode {
    if std::env::var_os("KYLER_INTERNAL_TOOL_WORKER").as_deref() == Some(OsStr::new("1")) {
        return match kyler_core::sandbox::worker::run_stdio().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Worker failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let args = Args::parse();
    let json = args.json;
    match execute(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if error.code != 0 {
                if json {
                    let _ = writeln!(
                        std::io::stdout().lock(),
                        "{}",
                        serde_json::json!({"type":"client_error","code":error.code,"message":error.message})
                    );
                } else {
                    eprintln!("Error: {error}");
                }
            }
            ExitCode::from(error.code)
        }
    }
}
