use kyler_config::loader;
use kyler_protocol::api::subagents::SubagentSettingsUpdate;
use kyler_server::Server;

#[tokio::test]
async fn settings_are_persistent_validated_and_do_not_change_the_active_runtime() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("config.toml");
    let text = format!(
        "[llm]\nbase_url=\"http://127.0.0.1:9/v1\"\napi_key=\"SYNTHETIC_TEST_KEY\"\nmodel=\"fixture\"\ndiscover_models=false\n[session]\nenabled=true\ndirectory={:?}\n[skills]\nenabled=false\n[subagent]\nenabled=true\nmax_concurrent_tasks=4\nmax_tasks_per_execution=12\nmax_queued_tasks=17\nmax_retained_tasks=40\nmax_steps=7\n",
        root.path().join("sessions").to_str().unwrap()
    );
    std::fs::write(&source, &text).unwrap();
    let config = loader::load_from(&source).unwrap();
    let server = Server::from_config(&config).await.unwrap();
    assert!(server.state.runs.subagent_settings().await.is_err());
    server
        .state
        .runs
        .set_model_source(source.clone(), Default::default());
    let mut member = kyler_protocol::api::gateway::Principal::local();
    member.role = kyler_protocol::api::gateway::Role::Member;
    let denied = kyler_server::routes::subagents::get(
        axum::extract::State(server.state.clone()),
        axum::Extension(member.clone()),
    )
    .await
    .unwrap_err();
    assert_eq!(denied.status, axum::http::StatusCode::FORBIDDEN);
    let denied = kyler_server::routes::subagents::update(
        axum::extract::State(server.state.clone()),
        axum::Extension(member),
        axum::Json(SubagentSettingsUpdate {
            enabled: false,
            max_concurrent_tasks: 2,
            max_tasks_per_execution: 8,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(denied.status, axum::http::StatusCode::FORBIDDEN);
    assert_eq!(std::fs::read_to_string(&source).unwrap(), text);
    let original = server.state.runs.subagent_settings().await.unwrap();
    assert!(!original.restart_required);
    assert_eq!(original.active.max_concurrent_tasks, 4);
    let update = SubagentSettingsUpdate {
        enabled: true,
        max_concurrent_tasks: 2,
        max_tasks_per_execution: 8,
    };
    let saved = server
        .state
        .runs
        .update_subagent_settings(update.clone())
        .await
        .unwrap();
    assert_eq!(saved.configured, update);
    assert!(saved.restart_required);
    assert_eq!(saved.active, original.active);
    let config = loader::load_from(&source).unwrap();
    assert_eq!(config.subagent.max_steps, 7);
    assert_eq!(config.subagent.max_queued_tasks, 17);
    assert_eq!(config.subagent.max_retained_tasks, 40);
    assert_eq!(config.llm.api_key.expose(), "SYNTHETIC_TEST_KEY");
    assert_eq!(config.subagent.max_concurrent_tasks, 2);
    let before_invalid = std::fs::read_to_string(&source).unwrap();
    for (concurrent, total) in [(0, 8), (33, 8), (2, 0), (2, 65)] {
        assert!(
            server
                .state
                .runs
                .update_subagent_settings(SubagentSettingsUpdate {
                    enabled: true,
                    max_concurrent_tasks: concurrent,
                    max_tasks_per_execution: total
                })
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&source).unwrap(), before_invalid);
    }
    let disabled = server
        .state
        .runs
        .update_subagent_settings(SubagentSettingsUpdate {
            enabled: false,
            ..update
        })
        .await
        .unwrap();
    assert!(!disabled.configured.enabled);
    assert!(disabled.active.enabled);
    assert!(disabled.restart_required);
    let restored = server
        .state
        .runs
        .update_subagent_settings(original.configured)
        .await
        .unwrap();
    assert!(!restored.restart_required);
    server
        .state
        .runs
        .update_subagent_settings(SubagentSettingsUpdate {
            enabled: true,
            max_concurrent_tasks: 2,
            max_tasks_per_execution: 8,
        })
        .await
        .unwrap();
    drop(server);
    let config = loader::load_from(&source).unwrap();
    let restarted = Server::from_config(&config).await.unwrap();
    restarted
        .state
        .runs
        .set_model_source(source, Default::default());
    assert!(
        !restarted
            .state
            .runs
            .subagent_settings()
            .await
            .unwrap()
            .restart_required
    );
    assert_eq!(
        restarted
            .state
            .runs
            .subagent_settings()
            .await
            .unwrap()
            .active
            .max_concurrent_tasks,
        2
    );
}

#[tokio::test]
async fn disabling_persisted_children_rejects_invalid_configuration_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("config.toml");
    let text = format!(
        "[llm]\nbase_url=\"http://127.0.0.1:9/v1\"\napi_key=\"SYNTHETIC_TEST_KEY\"\nmodel=\"fixture\"\ndiscover_models=false\n[session]\nenabled=true\ndirectory={:?}\n[skills]\nenabled=false\n[subagent]\npersist_sessions=true\n",
        root.path().join("sessions").to_str().unwrap()
    );
    std::fs::write(&source, &text).unwrap();
    let config = loader::load_from(&source).unwrap();
    let server = Server::from_config(&config).await.unwrap();
    server
        .state
        .runs
        .set_model_source(source.clone(), Default::default());
    assert!(
        server
            .state
            .runs
            .update_subagent_settings(SubagentSettingsUpdate {
                enabled: false,
                max_concurrent_tasks: 2,
                max_tasks_per_execution: 8
            })
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&source).unwrap(), text);
}
