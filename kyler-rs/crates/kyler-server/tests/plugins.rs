use serde_json::{Value, json};
use tokio::net::TcpListener;

use kyler_config::{AppConfig, SecretString, SkillSettings};
use kyler_server::Server;

#[tokio::test]
async fn plugins_are_managed_without_workspaces_or_sessions() {
    let root = tempfile::tempdir().unwrap();
    let mut config = AppConfig::default();
    config.llm.api_key = SecretString::new("PRIVATE_PLUGIN_FIXTURE");
    config.llm.base_url = "http://127.0.0.1:1/v1".into();
    config.llm.model = "fixture".into();
    config.session.enabled = true;
    config.session.directory = root.path().join("sessions");
    config.skills = SkillSettings {
        user_directory: root.path().join("skills"),
        state_file: root.path().join("skills-state.json"),
        ..Default::default()
    };
    config.server.access_token = SecretString::new("fixture-access");
    let server = Server::from_config(&config).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async {
        let _ = stopped.await;
    }));
    let http = reqwest::Client::builder()
        .default_headers(
            [(
                reqwest::header::AUTHORIZATION,
                "Bearer fixture-access".parse().unwrap(),
            )]
            .into_iter()
            .collect(),
        )
        .build()
        .unwrap();
    assert_eq!(
        reqwest::get(format!("{base}/v1/plugins/skills"))
            .await
            .unwrap()
            .status(),
        401
    );
    let url = format!("{base}/v1/plugins/skills");
    let skills: Value = http
        .get(&url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        skills
            .as_array()
            .unwrap()
            .iter()
            .any(|skill| skill["source"] == "builtin")
    );
    let added: Value = http.post(&url).json(&json!({"name":"without-workspace","scope":"user","files":{"SKILL.md":"---\nname: without-workspace\ndescription: A global user skill\n---\nHelp the user."},"expected_fingerprint":null})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(added["source"], "user");
    let details: Value = http
        .get(format!("{url}/{}", added["id"].as_str().unwrap()))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        details["instructions"]
            .as_str()
            .unwrap()
            .contains("Help the user")
    );
    assert_eq!(
        http.post(format!("{url}/{}/disable", added["id"].as_str().unwrap()))
            .json(&json!({"fingerprint":added["fingerprint"]}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let source = root.path().join("upload-source");
    std::fs::create_dir_all(source.join("assets")).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: uploaded-skill\ndescription: Uploaded user skill\n---\nUse uploaded resources.",
    )
    .unwrap();
    std::fs::write(source.join("assets/binary.dat"), [0, 255, 42]).unwrap();
    let package = kyler_core::skills::package_directory(&source).unwrap();
    assert_eq!(
        reqwest::Client::new()
            .post(format!("{url}/upload"))
            .body(package.clone())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let uploaded: Value = http
        .post(format!("{url}/upload"))
        .header("content-type", "application/zip")
        .body(package.clone())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(uploaded["source"], "user");
    assert_eq!(uploaded["status"], "enabled");
    assert_eq!(
        http.post(format!("{url}/upload"))
            .body(package)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let target = format!("{url}/{}", uploaded["id"].as_str().unwrap());
    assert_eq!(
        http.delete(&target)
            .json(&json!({"fingerprint":"stale"}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        http.delete(&target)
            .json(&json!({"fingerprint":uploaded["fingerprint"]}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(http.get(&target).send().await.unwrap().status(), 400);
    let builtin = skills
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["source"] == "builtin")
        .unwrap();
    assert_eq!(
        http.delete(format!("{url}/{}", builtin["id"].as_str().unwrap()))
            .json(&json!({"fingerprint":builtin["fingerprint"]}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let connectors = format!("{base}/v1/plugins/connectors");
    let connector: Value = http.post(&connectors).json(&json!({"id":"configured_only","transport":"streamable_http","url":"http://127.0.0.1:1/mcp","bearer_token_env":null})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(connector["status"], "disconnected");
    assert_eq!(connector["removable"], true);
    let listed: Value = http
        .get(&connectors)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["status"], "disconnected");
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    use std::io::Write;
    archive
        .start_file("connector.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive
        .write_all(br#"{"id":"uploaded_fixture","command":"server"}"#)
        .unwrap();
    archive
        .start_file("server", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
    let package = archive.finish().unwrap().into_inner();
    let upload = format!("{connectors}/upload");
    assert_eq!(
        http.post(&upload)
            .body("invalid zip")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let response = http
        .post(&upload)
        .body(package.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "{}", response.text().await.unwrap());
    assert_eq!(
        http.post(&upload)
            .body(package)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        http.post(format!("{connectors}/configured_only/connect"))
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    assert_eq!(
        http.post(format!("{connectors}/unknown/connect"))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    for id in ["configured_only", "uploaded_fixture"] {
        assert_eq!(
            http.delete(format!("{connectors}/{id}"))
                .send()
                .await
                .unwrap()
                .status(),
            204
        );
    }
    assert!(
        !root
            .path()
            .join("sessions/_connector_packages/uploaded_fixture")
            .exists()
    );
    let remaining: Value = http
        .get(&connectors)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(remaining.as_array().unwrap().is_empty());
    assert_eq!(
        http.delete(format!("{connectors}/unknown"))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let model = http
        .get(format!("{base}/v1/models/settings"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!model.contains("PRIVATE_PLUGIN_FIXTURE"));
    let sessions: Value = http
        .get(format!("{base}/v1/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(sessions["sessions"].as_array().unwrap().is_empty());
    let workspaces: Value = http
        .get(format!("{base}/v1/workspaces"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(workspaces.as_array().unwrap().is_empty());
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
