use std::path::PathBuf;

use tokio::{net::TcpListener, signal::unix::SignalKind};

use kyler_server::Server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = kyler_config::load()?;
    kyler_logging::init(config.logging.clone())?;
    let server = Server::from_config(&config).await?;
    let source = std::env::var_os("KYLER_CONFIG")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(kyler_config::loader::default_config_path)?;
    server
        .state
        .runs
        .set_model_source(source, Default::default());
    let listener = TcpListener::bind(config.server.bind).await?;
    tracing::info!(address=%listener.local_addr()?,"Kyler server listening");
    server
        .serve(listener, async {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(SignalKind::terminate()).expect("SIGTERM handler");
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        })
        .await
}
