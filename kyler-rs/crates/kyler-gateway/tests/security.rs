use kyler_gateway::security::{IssuedToken, Security};
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

fn internal_principal(token: &IssuedToken) -> Principal {
    Principal {
        id: token.info.principal_id.clone(),
        token_id: Some(token.info.id.clone()),
        role: token.info.role,
        scope: token.info.scope.clone(),
    }
}

#[tokio::test]
async fn rotation_is_atomic_and_rejects_legacy_roles() {
    let root = tempfile::tempdir().unwrap();
    let security = Security::open(root.path()).unwrap();
    let host = Principal::local();
    for role in [Role::Admin, Role::Member, Role::Viewer] {
        let legacy = security.issue(&host, request(role)).await.unwrap();
        let principal = internal_principal(&legacy);
        assert!(
            security
                .authenticate(&legacy.secret)
                .await
                .unwrap()
                .is_none()
        );
        assert!(security.rotate(&host, &legacy.info.id).await.is_err());
        assert!(
            security
                .issue(&principal, request(Role::Owner))
                .await
                .is_err()
        );
        assert!(security.revoke(&principal, &legacy.info.id).await.is_err());
    }
    let original = security.issue(&host, request(Role::Owner)).await.unwrap();
    let replacement = security.rotate(&host, &original.info.id).await.unwrap();
    assert_eq!(original.info.principal_id, replacement.info.principal_id);
    assert_eq!(replacement.info.role, Role::Owner);
    assert!(
        security
            .authenticate(&original.secret)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        security
            .authenticate(&replacement.secret)
            .await
            .unwrap()
            .is_some()
    );
    assert!(security.rotate(&host, &original.info.id).await.is_err());
    let audit = security.store.audit(None, 20).await.unwrap();
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
    let principal = internal_principal(&token);
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
    let mut expired = request(Role::Owner);
    expired.expires_at = Some(1);
    assert!(security.issue(&Principal::local(), expired).await.is_err());
    let mut expires = request(Role::Owner);
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
