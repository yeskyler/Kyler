//! Durable gateway credentials, resource ownership and append-first audit records.
use crate::Database;

use std::path::Path;

use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};

use kyler_protocol::api::gateway::{AuditRecord, TokenInfo};

#[derive(Clone)]
pub struct GatewayStore(pub Database);
pub struct TokenRecord {
    pub info: TokenInfo,
    pub digest: Vec<u8>,
}
impl GatewayStore {
    pub fn open(path: &Path) -> Result<Self> {
        let database = Database::open(path)?;
        database.migrate(1, |tx, _| {
            tx.execute_batch("CREATE TABLE tokens(id TEXT PRIMARY KEY, principal_id TEXT NOT NULL, info TEXT NOT NULL, digest BLOB NOT NULL, expires_at INTEGER, revoked_at INTEGER, last_used_at INTEGER);
                CREATE TABLE resource_owners(kind TEXT NOT NULL, resource TEXT NOT NULL, principal_id TEXT NOT NULL, PRIMARY KEY(kind,resource));
                CREATE INDEX resource_owner ON resource_owners(principal_id,kind);
                CREATE TABLE audit(id INTEGER PRIMARY KEY AUTOINCREMENT, request_id TEXT NOT NULL, principal_id TEXT, token_id TEXT, action TEXT NOT NULL, resource TEXT, started_at INTEGER NOT NULL, finished_at INTEGER, status INTEGER);
                CREATE INDEX audit_started ON audit(started_at);")?;
            Ok(())
        })?;
        Ok(Self(database))
    }
    fn insert(tx: &rusqlite::Transaction<'_>, record: &TokenRecord) -> Result<()> {
        let info = &record.info;
        tx.execute(
            "INSERT INTO tokens(id,principal_id,info,digest,expires_at) VALUES(?,?,?,?,?)",
            params![
                info.id,
                info.principal_id,
                serde_json::to_string(info)?,
                record.digest,
                info.expires_at
            ],
        )?;
        Ok(())
    }
    pub async fn create(
        &self,
        record: TokenRecord,
        previous: Option<String>,
        actor: String,
        now: i64,
    ) -> Result<()> {
        self.0.with_async(move |connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if let Some(id) = previous {
                ensure!(tx.execute("UPDATE tokens SET revoked_at=? WHERE id=? AND revoked_at IS NULL",params![now,id])? == 1,"Token is missing or already revoked");
            }
            Self::insert(&tx,&record)?;
            tx.execute("INSERT INTO audit(request_id,principal_id,action,resource,started_at,finished_at,status) VALUES(?,?,'token.issue',?,?,?,201)",params![uuid::Uuid::new_v4().to_string(),actor,record.info.id,now,now])?;
            tx.commit()?;
            Ok(())
        }).await
    }
    pub async fn token(&self, id: String) -> Result<Option<TokenRecord>> {
        self.0
            .with_async(move |connection| {
                let row = connection
                    .query_row(
                        "SELECT info,digest,revoked_at,last_used_at FROM tokens WHERE id=?",
                        [id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, Vec<u8>>(1)?,
                                row.get::<_, Option<i64>>(2)?,
                                row.get::<_, Option<i64>>(3)?,
                            ))
                        },
                    )
                    .optional()?;
                row.map(|(json, digest, revoked, last)| {
                    let mut info: TokenInfo = serde_json::from_str(&json)?;
                    info.revoked_at = revoked;
                    info.last_used_at = last;
                    Ok(TokenRecord { info, digest })
                })
                .transpose()
            })
            .await
    }
    pub async fn list(&self) -> Result<Vec<TokenInfo>> {
        self.0
            .with_async(|connection| {
                let rows = connection
                    .prepare("SELECT info,revoked_at,last_used_at FROM tokens ORDER BY rowid DESC")?
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows.into_iter()
                    .map(|(json, revoked, last)| {
                        let mut info: TokenInfo = serde_json::from_str(&json)?;
                        info.revoked_at = revoked;
                        info.last_used_at = last;
                        Ok(info)
                    })
                    .collect()
            })
            .await
    }
    pub async fn active(&self, id: String, now: i64) -> Result<bool> {
        self.0.with_async(move |c|Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM tokens WHERE id=? AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>?))",params![id,now],|r|r.get(0))?)).await
    }
    pub async fn touch(&self, id: String, now: i64) -> Result<()> {
        self.0.with_async(move |c|{c.execute("UPDATE tokens SET last_used_at=? WHERE id=? AND (last_used_at IS NULL OR last_used_at<?)",params![now,id,now-60000])?;Ok(())}).await
    }
    pub async fn revoke(&self, id: String, actor: String, now: i64) -> Result<bool> {
        self.0.with_async(move |c|{
            let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let changed=tx.execute("UPDATE tokens SET revoked_at=COALESCE(revoked_at,?) WHERE id=?",params![now,id])?>0;
            if changed {tx.execute("INSERT INTO audit(request_id,principal_id,action,resource,started_at,finished_at,status) VALUES(?,?,'token.revoke',?,?,?,200)",params![uuid::Uuid::new_v4().to_string(),actor,id,now,now])?;}
            tx.commit()?;Ok(changed)
        }).await
    }
    pub async fn bind(
        &self,
        kind: String,
        resource: String,
        principal: String,
        now: i64,
    ) -> Result<()> {
        self.0.with_async(move |c| {
            let tx=c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute("INSERT INTO resource_owners(kind,resource,principal_id) VALUES(?,?,?)",params![kind,resource,principal])?;
            tx.execute("INSERT INTO audit(request_id,principal_id,action,resource,started_at,finished_at,status) VALUES(?,?,?,?,?,?,200)",params![uuid::Uuid::new_v4().to_string(),principal,format!("{kind}.bind"),resource,now,now])?;
            tx.commit()?;Ok(())
        }).await
    }
    pub async fn owns(&self, kind: String, resource: String, principal: String) -> Result<bool> {
        self.0.with_async(move |c|Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM resource_owners WHERE kind=? AND resource=? AND principal_id=?)",params![kind,resource,principal],|r|r.get(0))?)).await
    }
    pub async fn begin(&self, record: AuditRecord) -> Result<i64> {
        self.0.with_async(move |c|{
            c.execute("INSERT INTO audit(request_id,principal_id,token_id,action,resource,started_at) VALUES(?,?,?,?,?,?)",params![record.request_id,record.principal_id,record.token_id,record.action,record.resource,record.started_at])?;
            Ok(c.last_insert_rowid())
        }).await
    }
    pub async fn finish(&self, id: i64, status: u16, now: i64) -> Result<()> {
        self.0
            .with_async(move |c| {
                c.execute(
                    "UPDATE audit SET status=?,finished_at=? WHERE id=?",
                    params![status, now, id],
                )?;
                Ok(())
            })
            .await
    }
    pub async fn audit(&self, before: Option<i64>, limit: usize) -> Result<Vec<AuditRecord>> {
        ensure!((1..=500).contains(&limit), "Audit limit must be in 1..=500");
        self.0.with_async(move |c|Ok(c.prepare("SELECT id,request_id,principal_id,token_id,action,resource,started_at,finished_at,status FROM audit WHERE (? IS NULL OR id<?) ORDER BY id DESC LIMIT ?")?.query_map(params![before,before,limit as i64],|r|Ok(AuditRecord{id:r.get(0)?,request_id:r.get(1)?,principal_id:r.get(2)?,token_id:r.get(3)?,action:r.get(4)?,resource:r.get(5)?,started_at:r.get(6)?,finished_at:r.get(7)?,status:r.get(8)?}))?.collect::<rusqlite::Result<_>>()?)).await
    }
}
