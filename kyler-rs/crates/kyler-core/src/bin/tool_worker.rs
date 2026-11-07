#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    kyler_core::sandbox::worker::run_stdio().await
}
