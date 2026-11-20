//! Explicit new-gateway setup for HTTP fixtures. No compatibility credentials.
pub async fn issue(server: &kyler_server::Server) -> String {
    server
        .state
        .runs
        .security
        .issue(
            &kyler_protocol::api::gateway::Principal::local(),
            kyler_protocol::api::gateway::CreateToken {
                name: "http-fixture".into(),
                role: kyler_protocol::api::gateway::Role::Owner,
                scope: Default::default(),
                expires_at: None,
            },
        )
        .await
        .unwrap()
        .secret
}
pub fn client(secret: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([])
        .default_headers(
            [(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {secret}").parse().unwrap(),
            )]
            .into_iter()
            .collect(),
        )
        .build()
        .unwrap()
}
