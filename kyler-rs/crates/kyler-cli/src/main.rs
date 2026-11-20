use std::{ffi::OsStr, io::Write, process::ExitCode};

use clap::Parser;

use kyler_cli::{args::Args, execute};

fn main() -> ExitCode {
    // Re-exec before creating threads rather than mutating the process environment.
    // MXC's host helpers resolve their commands through the inherited PATH.
    #[cfg(unix)]
    if let Err(error) = initialize_bundled_runtime() {
        eprintln!("Bundled runtime could not start: {error}");
        return ExitCode::from(3);
    }
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
    let json = args.json && !matches!(&args.command, Some(kyler_cli::args::Command::Acp));
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

#[cfg(unix)]
fn initialize_bundled_runtime() -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let worker = std::env::current_exe()?;
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let path = kyler_core::sandbox::paths::bundled_search_path(&worker, &existing)?;
    if path != existing {
        return Err(std::process::Command::new(worker)
            .args(std::env::args_os().skip(1))
            .env("PATH", path)
            .exec());
    }
    Ok(())
}
