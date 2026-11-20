//! Host-authorized transport creation. Protocol and lifecycle live in kyler-mcp.
use super::process;
use crate::{environment::ExecutionEnvironment, sandbox::NetworkAccess};

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Result, ensure};

use kyler_config::{McpServerSettings, McpTransport};
use kyler_mcp::{Connection, TransportFactory, TransportGuard};

pub(super) struct HostTransport {
    pub settings: McpServerSettings,
    pub environment: Option<Arc<ExecutionEnvironment>>,
    pub package_root: Option<PathBuf>,
}
impl TransportGuard for process::ProcessLease {
    fn stop(&self) {
        process::ProcessLease::stop(self);
    }
}
#[async_trait::async_trait]
impl TransportFactory for HostTransport {
    async fn connect(&self) -> Result<Connection> {
        match self.settings.transport {
            McpTransport::Stdio => {
                let environment = self.environment.clone().ok_or_else(|| {
                    anyhow::anyhow!("Stdio MCP requires a session workspace sandbox.")
                })?;
                ensure!(
                    environment.policy().process.allow_commands,
                    "Session policy denies command execution."
                );
                let mut vars = BTreeMap::new();
                for key in self.settings.env.keys() {
                    vars.insert(
                        key.clone(),
                        self.settings
                            .resolved_env
                            .get(key)
                            .ok_or_else(|| anyhow::anyhow!("MCP environment reference is missing"))?
                            .expose()
                            .to_owned(),
                    );
                }
                let (stream, lease) = process::launch(
                    &self.settings.command,
                    &self.settings.args,
                    vars,
                    environment,
                    self.package_root.as_deref(),
                )
                .await?;
                Connection::from_io(stream, Arc::new(lease)).await
            }
            McpTransport::StreamableHttp => {
                let mut http = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(10));
                // Loopback endpoints stay local. Explicit sandbox proxy and deny
                // policy below still takes precedence over ambient OS settings.
                if reqwest::Url::parse(&self.settings.url)
                    .ok()
                    .filter(|url| url.scheme() == "http")
                    .and_then(|url| {
                        url.host_str()
                            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
                    })
                    .is_some_and(|address| address.is_loopback())
                {
                    http = http.no_proxy();
                }
                let plaintext =
                    reqwest::Url::parse(&self.settings.url).is_ok_and(|url| url.scheme() == "http");
                let tls_proxy=["HTTP_PROXY","http_proxy","ALL_PROXY","all_proxy"].iter().any(|key|std::env::var(key).ok().is_some_and(|value|reqwest::Url::parse(&value).is_ok_and(|url|url.scheme()=="https")))
                    || self.environment.as_ref().is_some_and(|environment|matches!(&environment.policy().network,NetworkAccess::Proxy{url} if reqwest::Url::parse(url).is_ok_and(|url|url.scheme()=="https")));
                if plaintext && !tls_proxy {
                    http = http.tls_certs_only([]);
                }
                if let Some(environment) = &self.environment {
                    match &environment.policy().network {
                        NetworkAccess::Deny => {
                            anyhow::bail!("sandbox network policy denies MCP HTTP")
                        }
                        NetworkAccess::Proxy { url } => {
                            http = http.proxy(reqwest::Proxy::all(url)?)
                        }
                        _ => {}
                    }
                }
                let token = if self.settings.bearer_token_env.is_some() {
                    Some(
                        self.settings
                            .resolved_token
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("MCP token reference is missing"))?
                            .expose()
                            .to_owned(),
                    )
                } else {
                    None
                };
                Connection::from_http(http.build()?, &self.settings.url, token).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::SandboxPolicy;

    #[tokio::test]
    async fn host_transport_rejects_network_and_process_access_before_creating_a_transport() {
        let directory = tempfile::tempdir().unwrap();
        let mut policy = SandboxPolicy::read_only("transport-policy", directory.path());
        policy.process.allow_commands = false;
        let environment = Arc::new(
            ExecutionEnvironment::mxc(
                directory.path(),
                directory.path(),
                policy,
                std::env::current_exe().unwrap(),
            )
            .unwrap(),
        );
        for (transport, expected) in [
            (McpTransport::StreamableHttp, "network policy denies"),
            (McpTransport::Stdio, "denies command execution"),
        ] {
            let factory = HostTransport {
                settings: McpServerSettings {
                    transport,
                    url: "http://127.0.0.1:1/mcp".into(),
                    command: "/must-not-be-launched".into(),
                    ..Default::default()
                },
                environment: Some(environment.clone()),
                package_root: None,
            };
            let error = factory
                .connect()
                .await
                .err()
                .expect("host restriction must reject transport creation");
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }
}
