//! Gateway identity and resource policy. No HTTP types or provider credentials.
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};

use kyler_protocol::api::gateway::{CreateToken, Principal, Role, Scope, TokenInfo};
use kyler_store::gateway::{GatewayStore, TokenRecord};

#[derive(Clone)]
pub struct Security {
    pub store: GatewayStore,
}
/// Never derive Debug or persist this value. Only explicit creation returns the secret.
pub struct IssuedToken {
    pub info: TokenInfo,
    pub secret: String,
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn equal(a: &[u8], b: &[u8]) -> bool {
    let mut difference = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        difference |= (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    difference == 0
}
impl Security {
    pub fn database_path(sessions: &Path) -> PathBuf {
        sessions.join("_gateway.sqlite")
    }
    pub fn open(sessions: &Path) -> Result<Self> {
        Ok(Self {
            store: GatewayStore::open(&Self::database_path(sessions))?,
        })
    }
    pub fn token_id(secret: &str) -> Option<&str> {
        if secret.len() != 101 || !secret.starts_with("kyl_") {
            return None;
        }
        let (id, random) = secret[4..].split_once('_')?;
        (id.len() == 32
            && random.len() == 64
            && id
                .bytes()
                .chain(random.bytes())
                .all(|b| b.is_ascii_hexdigit()))
        .then_some(id)
    }
    pub async fn authenticate(&self, secret: &str) -> Result<Option<Principal>> {
        let Some(id) = Self::token_id(secret) else {
            return Ok(None);
        };
        let Some(record) = self.store.token(id.into()).await? else {
            return Ok(None);
        };
        let digest = Sha256::digest(secret.as_bytes());
        if !equal(&digest, &record.digest)
            || record.info.revoked_at.is_some()
            || record.info.expires_at.is_some_and(|expiry| expiry <= now())
        {
            return Ok(None);
        }
        self.store.touch(id.into(), now()).await?;
        Ok(Some(Principal {
            id: record.info.principal_id,
            token_id: Some(id.into()),
            role: record.info.role,
            scope: record.info.scope,
        }))
    }
    pub async fn active(&self, principal: &Principal) -> Result<bool> {
        match &principal.token_id {
            Some(id) => self.store.active(id.clone(), now()).await,
            None => Ok(principal.id == "host:local" && principal.role == Role::Owner),
        }
    }
    pub async fn issue(&self, actor: &Principal, mut request: CreateToken) -> Result<IssuedToken> {
        ensure!(
            actor.role == Role::Owner && self.active(actor).await?,
            "Owner authorization is required"
        );
        Self::validate(&mut request)?;
        let issued = Self::mint(request, uuid::Uuid::new_v4().to_string())?;
        self.persist(&issued, None, actor).await?;
        Ok(issued)
    }
    pub async fn rotate(&self, actor: &Principal, id: &str) -> Result<IssuedToken> {
        ensure!(
            actor.role == Role::Owner && self.active(actor).await?,
            "Owner authorization is required"
        );
        let old = self
            .store
            .token(id.into())
            .await?
            .ok_or_else(|| anyhow::anyhow!("Token not found"))?;
        ensure!(old.info.revoked_at.is_none(), "Token is already revoked");
        let mut request = CreateToken {
            name: old.info.name,
            role: old.info.role,
            scope: old.info.scope,
            expires_at: old.info.expires_at,
        };
        let original_scope = request.scope.clone();
        Self::validate(&mut request)?;
        ensure!(
            request.scope == original_scope,
            "Workspace binding changed; issue a new scoped token"
        );
        let issued = Self::mint(request, old.info.principal_id)?;
        self.persist(&issued, Some(id.into()), actor).await?;
        Ok(issued)
    }
    async fn persist(
        &self,
        issued: &IssuedToken,
        previous: Option<String>,
        actor: &Principal,
    ) -> Result<()> {
        self.store
            .create(
                TokenRecord {
                    info: issued.info.clone(),
                    digest: Sha256::digest(issued.secret.as_bytes()).to_vec(),
                },
                previous,
                actor.id.clone(),
                now(),
            )
            .await
    }
    fn validate(request: &mut CreateToken) -> Result<()> {
        ensure!(
            !request.name.trim().is_empty()
                && request.name.len() <= 128
                && !request.name.chars().any(char::is_control),
            "Token name must contain 1..=128 bytes without control characters"
        );
        ensure!(
            request.expires_at.is_none_or(|expiry| expiry > now()),
            "Token expiry must be in the future"
        );
        ensure!(
            [
                request.scope.workspaces.len(),
                request.scope.models.len(),
                request.scope.sessions.len()
            ]
            .into_iter()
            .all(|n| n <= 256),
            "Scope has too many entries"
        );
        for id in request.scope.models.iter().chain(&request.scope.sessions) {
            ensure!(
                !id.is_empty()
                    && id.len() <= 256
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.:-/".contains(&b)),
                "Invalid resource identifier"
            );
        }
        for root in &mut request.scope.workspaces {
            let path = Path::new(root);
            ensure!(
                path.is_absolute() && path.is_dir(),
                "Workspace grants must be existing absolute directories"
            );
            *root = path
                .canonicalize()?
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Workspace path must be UTF-8"))?
                .into();
        }
        ensure!(
            !request.role.administrative() || request.scope == Scope::default(),
            "Administrative tokens cannot carry partial scopes"
        );
        Ok(())
    }
    fn mint(request: CreateToken, principal_id: String) -> Result<IssuedToken> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut random = [0u8; 32];
        getrandom::fill(&mut random)
            .map_err(|_| anyhow::anyhow!("Secure token generation failed"))?;
        let random = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        Ok(IssuedToken {
            secret: format!("kyl_{id}_{random}"),
            info: TokenInfo {
                id,
                principal_id,
                name: request.name,
                role: request.role,
                scope: request.scope,
                created_at: now(),
                expires_at: request.expires_at,
                revoked_at: None,
                last_used_at: None,
            },
        })
    }
    pub async fn revoke(&self, actor: &Principal, id: &str) -> Result<bool> {
        ensure!(
            actor.role == Role::Owner && self.active(actor).await?,
            "Owner authorization is required"
        );
        self.store.revoke(id.into(), actor.id.clone(), now()).await
    }
    pub async fn bind_session(&self, principal: &Principal, id: &str) -> Result<()> {
        self.store
            .bind("session".into(), id.into(), principal.id.clone(), now())
            .await
    }
    pub fn workspace_allowed(principal: &Principal, path: Option<&Path>) -> bool {
        if principal.scope.workspaces.is_empty() {
            return true;
        }
        path.and_then(|p| p.canonicalize().ok())
            .is_some_and(|path| {
                principal
                    .scope
                    .workspaces
                    .iter()
                    .any(|root| path.starts_with(root))
            })
    }
    pub fn model_allowed(principal: &Principal, profile: &str) -> bool {
        principal.scope.models.is_empty()
            || principal.scope.models.iter().any(|model| model == profile)
    }
    pub async fn session_allowed(
        &self,
        principal: &Principal,
        id: &str,
        workspace: Option<&Path>,
    ) -> Result<bool> {
        if !Self::workspace_allowed(principal, workspace) {
            return Ok(false);
        }
        Ok(principal.role.administrative()
            || principal.scope.sessions.iter().any(|session| session == id)
            || self
                .store
                .owns("session".into(), id.into(), principal.id.clone())
                .await?)
    }
}
