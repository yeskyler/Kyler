use kyler_gateway::security::Security;
use kyler_protocol::api::gateway::{CreateToken, Principal, Role, Scope};

fn request(role: Role) -> CreateToken {
    CreateToken {
        name: "test-client".into(),
        role,
        scope: Scope::default(),
        expires_at: None,
    }
}

#[tokio::test]
async fn only_issued_tokens_authenticate_and_restart_does_not_revive_revocation() {
    let root = tempfile::tempdir().unwrap();
    let security = Security::open(root.path()).unwrap();
    for old in ["", "old-shared-token", "Bearer anything", "kyl_not_a_token"] {
        assert!(security.authenticate(old).await.unwrap().is_none());
    }
    let issued = security
        .issue(&Principal::local(), request(Role::Owner))
        .await
        .unwrap();
    let actor = security
        .authenticate(&issued.secret)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actor.id, issued.info.principal_id);
    assert!(
        security.store.list().await.unwrap()[0]
            .last_used_at
            .is_some()
    );
    let stored = security
        .store
        .token(issued.info.id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.digest.len(), 32);
    assert!(
        !serde_json::to_string(&stored.info)
            .unwrap()
            .contains(&issued.secret)
    );
    let mut wrong = issued.secret.clone().into_bytes();
    let last = wrong.last_mut().unwrap();
    *last = if *last == b'a' { b'b' } else { b'a' };
    assert!(
        security
            .authenticate(std::str::from_utf8(&wrong).unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        security
            .revoke(&Principal::local(), &issued.info.id)
            .await
            .unwrap()
    );
    drop(security);
    let reopened = Security::open(root.path()).unwrap();
    assert!(
        reopened
            .authenticate(&issued.secret)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!reopened.active(&actor).await.unwrap());
}

#[tokio::test]
async fn rotation_is_atomic_retains_identity_and_does_not_escalate() {
    let root = tempfile::tempdir().unwrap();
    let security = Security::open(root.path()).unwrap();
    let original = security
        .issue(&Principal::local(), request(Role::Member))
        .await
        .unwrap();
    let member = security
        .authenticate(&original.secret)
        .await
        .unwrap()
        .unwrap();
    assert!(security.issue(&member, request(Role::Owner)).await.is_err());
    assert!(security.revoke(&member, &original.info.id).await.is_err());
    security.bind_session(&member, "session-one").await.unwrap();
    let replacement = security
        .rotate(&Principal::local(), &original.info.id)
        .await
        .unwrap();
    assert_eq!(original.info.principal_id, replacement.info.principal_id);
    assert_eq!(replacement.info.role, Role::Member);
    assert!(
        security
            .authenticate(&original.secret)
            .await
            .unwrap()
            .is_none()
    );
    let principal = security
        .authenticate(&replacement.secret)
        .await
        .unwrap()
        .unwrap();
    assert!(
        security
            .session_allowed(&principal, "session-one", None)
            .await
            .unwrap()
    );
    assert!(
        security
            .rotate(&Principal::local(), &original.info.id)
            .await
            .is_err()
    );
    let audit = security.store.audit(None, 20).await.unwrap();
    assert_eq!(
        audit
            .iter()
            .filter(|record| record.action == "token.issue")
            .count(),
        2
    );
    assert!(
        !serde_json::to_string(&audit)
            .unwrap()
            .contains(&original.secret)
    );
}

#[tokio::test]
async fn ownership_workspace_and_profile_grants_are_intersected() {
    let root = tempfile::tempdir().unwrap();
    let security = Security::open(root.path()).unwrap();
    let workspace = root.path().join("allowed");
    let outside = root.path().join("outside");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let mut grant = request(Role::Member);
    grant.scope = Scope {
        workspaces: vec![workspace.to_str().unwrap().into()],
        sessions: vec!["shared".into()],
        models: vec!["approved-profile".into()],
    };
    let token = security.issue(&Principal::local(), grant).await.unwrap();
    let principal = security.authenticate(&token.secret).await.unwrap().unwrap();
    security.bind_session(&principal, "owned").await.unwrap();
    assert!(
        security
            .session_allowed(&principal, "owned", Some(&workspace))
            .await
            .unwrap()
    );
    assert!(
        security
            .session_allowed(&principal, "shared", Some(&workspace))
            .await
            .unwrap()
    );
    assert!(
        !security
            .session_allowed(&principal, "other", Some(&workspace))
            .await
            .unwrap()
    );
    assert!(
        !security
            .session_allowed(&principal, "owned", Some(&outside))
            .await
            .unwrap()
    );
    assert!(
        !security
            .session_allowed(&principal, "owned", None)
            .await
            .unwrap()
    );
    assert!(Security::model_allowed(&principal, "approved-profile"));
    assert!(!Security::model_allowed(&principal, "other-profile"));
    #[cfg(unix)]
    {
        let link = workspace.join("escape");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(!Security::workspace_allowed(&principal, Some(&link)));
    }
}

#[tokio::test]
async fn expired_tokens_and_invalid_delegations_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let security = Security::open(root.path()).unwrap();
    let mut grant = request(Role::Admin);
    grant.scope.sessions.push("scoped".into());
    assert!(security.issue(&Principal::local(), grant).await.is_err());
    let mut expired = request(Role::Viewer);
    expired.expires_at = Some(1);
    assert!(security.issue(&Principal::local(), expired).await.is_err());
    let mut expires = request(Role::Viewer);
    expires.expires_at = Some(chrono::Utc::now().timestamp_millis() + 200);
    let token = security.issue(&Principal::local(), expires).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        security
            .authenticate(&token.secret)
            .await
            .unwrap()
            .is_none()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn rotation_never_retargets_a_workspace_grant_through_a_new_symlink() {
    let root = tempfile::tempdir().unwrap();
    let security = Security::open(root.path()).unwrap();
    let workspace = root.path().join("workspace");
    let outside = root.path().join("outside");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let mut grant = request(Role::Member);
    grant
        .scope
        .workspaces
        .push(workspace.to_str().unwrap().into());
    let token = security.issue(&Principal::local(), grant).await.unwrap();
    std::fs::rename(&workspace, root.path().join("original")).unwrap();
    std::os::unix::fs::symlink(&outside, &workspace).unwrap();
    assert!(
        security
            .rotate(&Principal::local(), &token.info.id)
            .await
            .is_err()
    );
    let current = security.store.token(token.info.id).await.unwrap().unwrap();
    assert!(current.info.revoked_at.is_none());
    let principal = security.authenticate(&token.secret).await.unwrap().unwrap();
    assert!(!Security::workspace_allowed(&principal, Some(&outside)));
}
