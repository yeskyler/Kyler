use crate::{CliError, Result, output};

pub async fn execute(json: bool) -> Result<()> {
    let backend = if cfg!(target_os = "linux") {
        "Bubblewrap"
    } else if cfg!(target_os = "macos") {
        "Seatbelt"
    } else {
        return Err(CliError::new(
            2,
            "Native sandboxes currently support Linux and macOS only",
        ));
    };
    if !json {
        output::text(&format!(
            "Checking native sandbox as the current user...\nPlatform: {} {}\nBackend: {backend}\n",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))?;
    }
    let worker = std::env::current_exe().map_err(CliError::io)?;
    let checks = kyler_core::sandbox::diagnostics::check(&worker).await.map_err(|error| CliError::new(3, format!("Sandbox check failed: {error:#}. On Linux, check Bubblewrap/network dependencies, user namespaces, AppArmor and container host restrictions. No host security settings were changed.")))?;
    if json {
        output::json(
            &serde_json::json!({"type":"sandbox_diagnostics","backend":backend,"checks":checks}),
        )
    } else {
        for check in checks {
            output::text(&format!(
                "{}: process, workspace read/write, read-only/denied/outside paths and host loopback protection — OK\n",
                check.mode
            ))?;
        }
        output::text("Sandbox environment ready. No model requests were made.\n")
    }
}
