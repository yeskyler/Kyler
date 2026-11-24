use std::path::Path;

use crate::{Binding, Incoming, bindings::pairing_digest};
use anyhow::Result;
use rusqlite::{OptionalExtension, Transaction, params};
use uuid::Uuid;

use kyler_store::Database;

pub struct Job {
    pub id: i64,
    pub binding: Binding,
    pub input: String,
    pub run: Option<String>,
    pub notified: bool,
    pub dispatched_at: Option<i64>,
    reply_key: Option<String>,
}

pub struct Reply {
    pub id: i64,
    pub chat: String,
    pub text: String,
    pub token: Option<String>,
    pub receipt: Option<ReplyReceipt>,
}

pub struct ReplyReceipt {
    pub message_id: String,
    pub sequence: i64,
    pub expires: i64,
    pub context_token: Option<String>,
}

pub struct ChannelStore {
    database: Database,
    bot: String,
}

fn enqueue(
    tx: &Transaction<'_>,
    bot: &str,
    chat: &str,
    text: &str,
    token: Option<&str>,
    key: Option<&str>,
) -> Result<()> {
    // A conservative UTF-16 limit also fits Telegram's character limit for emoji.
    let qq = bot.starts_with("qqbot:");
    let callback = bot.starts_with("wecom_callback:");
    let bytes = callback || bot.starts_with("wecom:");
    let limit = if callback {
        2048
    } else if bot.starts_with("discord:") {
        2000
    } else if qq {
        1700
    } else if bot.starts_with("sms:") {
        1500
    } else {
        4096
    };
    let insert = |text: &str| -> Result<bool> {
        let sequence = if let Some(key) = key {
            let sequence: Option<i64> = tx
                .query_row(
                    "UPDATE updates SET reply_count=reply_count+1 WHERE bot=?1 AND update_id=?2
                 AND reply_count<5 AND received>?3 RETURNING reply_count",
                    params![bot, key, chrono::Utc::now().timestamp() - 3600],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(sequence) = sequence else {
                return Ok(false);
            };
            Some(sequence)
        } else {
            None
        };
        tx.execute(
            "INSERT INTO replies(bot,chat,text,token,reply_key,reply_seq) VALUES (?1,?2,?3,?4,?5,?6)",
            params![bot,chat,text,token,key,sequence],
        )?;
        Ok(true)
    };
    let mut start = 0;
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        let width = if bytes { ch.len_utf8() } else { ch.len_utf16() };
        units += width;
        if units > limit {
            if !insert(&text[start..index])? {
                return Ok(());
            }
            start = index;
            units = width;
        }
    }
    if start < text.len() {
        insert(&text[start..])?;
    }
    Ok(())
}

impl ChannelStore {
    pub fn open(path: &Path, bot: String) -> Result<Self> {
        let database = Database::open(path)?;
        database.migrate(3, |tx, version| {
            if version == 0 {
                tx.execute_batch("
                CREATE TABLE pairs(bot INTEGER, digest BLOB, token TEXT, workspace TEXT, expires INTEGER,
                    PRIMARY KEY(bot,digest));
                CREATE TABLE bindings(bot INTEGER, chat INTEGER, token TEXT, workspace TEXT, session TEXT,
                    PRIMARY KEY(bot,chat));
                CREATE TABLE updates(bot INTEGER, update_id INTEGER, received INTEGER,
                    PRIMARY KEY(bot,update_id));
                CREATE TABLE jobs(id INTEGER PRIMARY KEY AUTOINCREMENT, bot INTEGER, chat INTEGER,
                    token TEXT, workspace TEXT, session TEXT, input TEXT,
                    run TEXT, dispatched_at INTEGER, notified INTEGER NOT NULL DEFAULT 0);
                CREATE INDEX jobs_by_chat ON jobs(bot,chat,id);
                CREATE TABLE replies(id INTEGER PRIMARY KEY AUTOINCREMENT, bot INTEGER, chat INTEGER,
                    text TEXT, token TEXT, retry_after INTEGER NOT NULL DEFAULT 0, attempts INTEGER NOT NULL DEFAULT 0);
                CREATE INDEX replies_by_chat ON replies(bot,chat,id);
            ")?;
            }
            if version < 2 {
            tx.execute_batch("
                ALTER TABLE jobs ADD COLUMN reply_key TEXT;
                ALTER TABLE replies ADD COLUMN reply_key TEXT;
                ALTER TABLE replies ADD COLUMN reply_seq INTEGER;
                ALTER TABLE updates ADD COLUMN reply_count INTEGER NOT NULL DEFAULT 0;
            ")?;
            }
            tx.execute_batch("
                ALTER TABLE updates ADD COLUMN reply_context TEXT;
                CREATE TABLE adapter_state(bot TEXT NOT NULL,key TEXT NOT NULL,value TEXT NOT NULL, PRIMARY KEY(bot,key));
            ")?;
            Ok(())
        })?;
        Ok(Self { database, bot })
    }

    pub async fn pair(&self, token: String, workspace: String) -> Result<String> {
        let code = Uuid::new_v4().simple().to_string();
        let digest = pairing_digest(&code);
        let bot = self.bot.clone();
        self.database
            .with_async(move |db| {
                let now = chrono::Utc::now().timestamp();
                db.execute("DELETE FROM pairs WHERE expires <= ?1", [now])?;
                db.execute(
                    "INSERT INTO pairs VALUES (?1,?2,?3,?4,?5)",
                    params![bot, digest, token, workspace, now + 300],
                )?;
                Ok(())
            })
            .await?;
        Ok(code)
    }

    pub async fn bindings(&self) -> Result<Vec<Binding>> {
        let bot = self.bot.clone();
        self.database
            .with_async(move |db| {
                let mut query = db.prepare(
                    "SELECT CAST(chat AS TEXT),token,workspace,session FROM bindings WHERE bot=?1 ORDER BY chat",
                )?;
                Ok(query
                    .query_map([bot], |row| {
                        Ok(Binding {
                            chat: row.get(0)?,
                            token: row.get(1)?,
                            workspace: row.get(2)?,
                            session: row.get(3)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?)
            })
            .await
    }

    pub async fn remove(&self, chat: String, token: String) -> Result<()> {
        let bot = self.bot.clone();
        self.database
            .with_async(move |db| {
                let tx = db.transaction()?;
                let removed = tx.execute(
                    "DELETE FROM bindings WHERE bot=?1 AND chat=?2 AND token=?3",
                    params![bot, chat, token],
                )?;
                if removed == 0 {
                    return Ok(());
                }
                tx.execute(
                    "DELETE FROM jobs WHERE bot=?1 AND chat=?2",
                    params![bot, chat],
                )?;
                tx.execute(
                    "DELETE FROM replies WHERE bot=?1 AND chat=?2",
                    params![bot, chat],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await
    }

    pub async fn ingest(&self, messages: Vec<Incoming>) -> Result<()> {
        self.ingest_batch(
            messages
                .into_iter()
                .map(|message| (message, None))
                .collect(),
            None,
        )
        .await
    }

    pub async fn ingest_with_reply_context(
        &self,
        message: Incoming,
        context: String,
    ) -> Result<()> {
        self.ingest_batch(vec![(message, Some(context))], None)
            .await
    }

    pub(crate) async fn cursor(&self) -> Result<String> {
        let bot = self.bot.clone();
        self.database
            .with_async(move |db| {
                Ok(db
                    .query_row(
                        "SELECT value FROM adapter_state WHERE bot=?1 AND key='cursor'",
                        [bot],
                        |row| row.get(0),
                    )
                    .optional()?
                    .unwrap_or_default())
            })
            .await
    }

    pub(crate) async fn ingest_batch(
        &self,
        messages: Vec<(Incoming, Option<String>)>,
        cursor: Option<String>,
    ) -> Result<()> {
        if messages.is_empty() && cursor.is_none() {
            return Ok(());
        }
        let bot = self.bot.clone();
        self.database.with_async(move |db| {
            let tx = db.transaction()?;
            let now = chrono::Utc::now().timestamp();
            tx.execute("DELETE FROM updates WHERE received < ?1", [now-7*86400])?;
            tx.execute("DELETE FROM pairs WHERE expires <= ?1", [now])?;
            for (message, reply_context) in messages {
                let Incoming { update, chat, text } = message;
                if tx.execute("INSERT OR IGNORE INTO updates(bot,update_id,received,reply_context) VALUES (?1,?2,?3,?4)", params![bot,update,now,reply_context])? == 0 {
                    continue;
                }
                let reply_key = (reply_context.is_some() || bot.starts_with("qqbot:")).then_some(update.as_str());
                let (command, argument) = text.trim().split_once(char::is_whitespace).unwrap_or((text.trim(), ""));
                let action = command.strip_prefix('/').or_else(|| command.strip_prefix('!'));

                if action == Some("pair") {
                    let pair: Option<(String, String)> = tx.query_row(
                        "DELETE FROM pairs WHERE bot=?1 AND digest=?2
                         AND NOT EXISTS (SELECT 1 FROM bindings WHERE bot=?1 AND chat=?3)
                         RETURNING token,workspace",
                        params![bot,pairing_digest(argument.trim()),chat], |r| Ok((r.get(0)?,r.get(1)?)),
                    ).optional()?;
                    if let Some((token, workspace)) = pair {
                        tx.execute("INSERT INTO bindings VALUES (?1,?2,?3,?4,?5)",
                            params![bot,chat,token,workspace,Uuid::new_v4().to_string()])?;
                        enqueue(&tx,&bot,&chat,"Paired. Send a message to start a chat. Use !new for a new chat or !cancel to stop a run.", None, reply_key)?;
                    } else {
                        enqueue(&tx,&bot,&chat,"Pairing code invalid or expired, or this chat is already paired.", None, reply_key)?;
                    }
                } else if action == Some("start") {
                    enqueue(&tx,&bot,&chat,"Open Channels in the Kyler Web sidebar and send !pair followed by your pairing code.", None, reply_key)?;
                } else {
                    let text = match action {
                        Some("new") if argument.trim().is_empty() => "/new",
                        Some("cancel") if argument.trim().is_empty() => "/cancel",
                        _ => &text,
                    };
                    let bound = tx.execute("INSERT INTO jobs(bot,chat,token,workspace,session,input,reply_key)
                        SELECT bot,chat,token,workspace,session,?3,?4 FROM bindings WHERE bot=?1 AND chat=?2",
                        params![bot,chat,text,reply_key])?;
                    if bound == 0 {
                        enqueue(&tx,&bot,&chat,"Pair this chat from Kyler Web before sending a message.", None, reply_key)?;
                    }
                }
            }
            if let Some(cursor) = cursor {
                tx.execute("INSERT INTO adapter_state VALUES (?1,'cursor',?2)
                    ON CONFLICT(bot,key) DO UPDATE SET value=excluded.value", params![bot,cursor])?;
            }
            tx.commit()?;
            Ok(())
        }).await
    }

    pub async fn jobs(&self) -> Result<Vec<Job>> {
        let bot = self.bot.clone();
        self.database
            .with_async(move |db| {
                // Let cancellation pass queued prompts, while dispatching one prompt per chat.
                let mut query = db.prepare(
                    "SELECT id,CAST(chat AS TEXT),token,workspace,session,input,run,notified,dispatched_at,reply_key
                FROM jobs j WHERE j.bot=?1 AND (trim(j.input)='/cancel' OR j.id=(
                    SELECT min(id) FROM jobs WHERE bot=j.bot AND chat=j.chat
                )) ORDER BY (trim(input)='/cancel') DESC,id",
                )?;
                Ok(query
                    .query_map([bot], |r| {
                        Ok(Job {
                            id: r.get(0)?,
                            binding: Binding {
                                chat: r.get(1)?,
                                token: r.get(2)?,
                                workspace: r.get(3)?,
                                session: r.get(4)?,
                            },
                            input: r.get(5)?,
                            run: r.get(6)?,
                            notified: r.get(7)?,
                            dispatched_at: r.get(8)?,
                            reply_key: r.get(9)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?)
            })
            .await
    }

    pub async fn mark_dispatch(&self, id: i64) -> Result<bool> {
        self.database
            .with_async(move |db| {
                Ok(db.execute(
                    "UPDATE jobs SET dispatched_at=coalesce(dispatched_at, ?2) WHERE id=?1",
                    params![id, chrono::Utc::now().timestamp_millis()],
                )? != 0)
            })
            .await
    }

    pub async fn clear_dispatch(&self, id: i64) -> Result<()> {
        self.database
            .with_async(move |db| {
                db.execute("UPDATE jobs SET dispatched_at=NULL WHERE id=?1", [id])?;
                Ok(())
            })
            .await
    }

    pub async fn set_run(&self, id: i64, run: String) -> Result<()> {
        self.database
            .with_async(move |db| {
                db.execute("UPDATE jobs SET run=?2 WHERE id=?1", params![id, run])?;
                Ok(())
            })
            .await
    }

    pub async fn complete(
        &self,
        job: &Job,
        text: String,
        new_session: bool,
        notice: bool,
    ) -> Result<()> {
        let (bot, id, chat, token, old_session) = (
            self.bot.clone(),
            job.id,
            job.binding.chat.clone(),
            job.binding.token.clone(),
            job.binding.session.clone(),
        );
        let reply_key = job.reply_key.clone();
        self.database.with_async(move |db| {
            let tx = db.transaction()?;
            let changed = if notice {
                tx.execute("UPDATE jobs SET notified=1 WHERE id=?1 AND notified=0", [id])?
            } else {
                tx.execute("DELETE FROM jobs WHERE id=?1", [id])?
            };
            if changed > 0 {
                if new_session {
                    let session = Uuid::new_v4().to_string();
                    tx.execute("UPDATE bindings SET session=?4 WHERE bot=?1 AND chat=?2 AND token=?3",
                        params![bot,chat,token,session])?;
                    tx.execute("UPDATE jobs SET session=?5 WHERE bot=?1 AND chat=?2 AND token=?3 AND session=?4 AND id>?6",
                        params![bot,chat,token,old_session,session,id])?;
                }
                enqueue(&tx,&bot,&chat,&text,Some(&token),reply_key.as_deref())?;
            }
            tx.commit()?;
            Ok(())
        }).await
    }

    pub async fn next_reply(&self) -> Result<Option<Reply>> {
        let bot = self.bot.clone();
        self.database
            .with_async(move |db| {
                Ok(db
                    .query_row(
                        "SELECT r.id,CAST(r.chat AS TEXT),r.text,r.token,r.reply_key,r.reply_seq,u.received,u.reply_context
                FROM replies r LEFT JOIN updates u ON u.bot=r.bot AND u.update_id=r.reply_key
                WHERE r.bot=?1 AND r.retry_after<=?2 AND r.id=(
                    SELECT min(id) FROM replies WHERE bot=r.bot AND chat=r.chat
                ) ORDER BY r.id LIMIT 1",
                        params![bot, chrono::Utc::now().timestamp()],
                        |r| {
                            Ok(Reply {
                                id: r.get(0)?,
                                chat: r.get(1)?,
                                text: r.get(2)?,
                                token: r.get(3)?,
                                receipt: r.get::<_,Option<String>>(4)?.zip(r.get::<_,Option<i64>>(5)?)
                                    .map(|(message_id, sequence)| -> rusqlite::Result<_> {
                                        Ok(ReplyReceipt {message_id, sequence, expires:r.get::<_,Option<i64>>(6)?.unwrap_or_default()+3600, context_token:r.get(7)?})
                                    }).transpose()?,
                            })
                        },
                    )
                    .optional()?)
            })
            .await
    }

    pub async fn delivery(&self, id: i64, retry: bool) -> Result<()> {
        let bot = self.bot.clone();
        self.database.with_async(move |db| {
            if retry {
                db.execute("UPDATE replies SET attempts=attempts+1, retry_after=?2+min(300, 5*(1<<min(attempts,6))) WHERE id=?1 AND bot=?3",
                    params![id,chrono::Utc::now().timestamp(),bot])?;
            } else {
                db.execute("DELETE FROM replies WHERE id=?1 AND bot=?2", params![id,bot])?;
            }
            Ok(())
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(update: i64, chat: i64, text: impl Into<String>) -> Incoming {
        Incoming {
            update: update.to_string(),
            chat: chat.to_string(),
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn pairing_is_single_use_and_duplicate_updates_survive_reopening() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("channels.sqlite");
        let store = ChannelStore::open(&file, "1".into())?;
        let code = store.pair("gateway-id".into(), "/workspace".into()).await?;
        let pair = format!("/pair {code}");
        store
            .ingest(vec![
                message(1, 10, &pair),
                message(2, 20, &pair),
                message(3, 10, "first"),
            ])
            .await?;
        drop(store);

        let store = ChannelStore::open(&file, "1".into())?;
        store.ingest(vec![message(3, 10, "first")]).await?;
        let bindings = store.bindings().await?;
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].chat, "10");
        assert_eq!(bindings[0].token, "gateway-id");
        assert_eq!(bindings[0].workspace, "/workspace");
        let jobs = store.jobs().await?;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].input, "first");
        assert!(store.mark_dispatch(jobs[0].id).await?);
        store.set_run(jobs[0].id, "existing-run".into()).await?;
        drop(store);

        let store = ChannelStore::open(&file, "1".into())?;
        let jobs = store.jobs().await?;
        assert_eq!(jobs[0].run.as_deref(), Some("existing-run"));
        store
            .complete(&jobs[0], "result".into(), false, false)
            .await?;
        assert!(store.jobs().await?.is_empty());
        for _ in 0..2 {
            let reply = store.next_reply().await?.unwrap();
            store.delivery(reply.id, false).await?;
        }
        let reply = store.next_reply().await?.unwrap();
        assert_eq!(reply.text, "result");
        assert_eq!(reply.token.as_deref(), Some("gateway-id"));
        store.delivery(reply.id, true).await?;
        assert!(store.next_reply().await?.is_none());
        assert!(store.jobs().await?.is_empty());
        store.remove("10".into(), "gateway-id".into()).await?;
        assert!(store.bindings().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn weixin_cursor_and_original_context_survive_restart() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("channels.sqlite");
        let store = ChannelStore::open(&path, "weixin:test".into())?;
        store
            .ingest_batch(
                vec![
                    (message(1, 10, "!start"), Some("first-context".into())),
                    (message(2, 10, "!start"), Some("new-context".into())),
                ],
                Some("cursor-two".into()),
            )
            .await?;
        drop(store);

        let store = ChannelStore::open(&path, "weixin:test".into())?;
        assert_eq!(store.cursor().await?, "cursor-two");
        let reply = store.next_reply().await?.unwrap();
        assert_eq!(
            reply.receipt.unwrap().context_token.as_deref(),
            Some("first-context")
        );
        store.delivery(reply.id, false).await?;
        let reply = store.next_reply().await?.unwrap();
        assert_eq!(
            reply.receipt.unwrap().context_token.as_deref(),
            Some("new-context")
        );
        Ok(())
    }

    #[tokio::test]
    async fn qq_replies_keep_original_message_and_a_bounded_stable_sequence() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("channels.sqlite");
        let store = ChannelStore::open(&file, "qqbot:bot".into())?;
        let code = store.pair("credential".into(), "/workspace".into()).await?;
        let post = |id: &str, text: String| Incoming {
            update: format!("qq_{id}"),
            chat: "qq_user".into(),
            text,
        };
        store
            .ingest(vec![
                post("pair", format!("!pair {code}")),
                post("first", "run one".into()),
                post("second", "run two".into()),
            ])
            .await?;
        let first = store.jobs().await?.remove(0);
        store
            .complete(&first, "x".repeat(20_000), false, false)
            .await?;
        drop(store);

        let store = ChannelStore::open(&file, "qqbot:bot".into())?;
        let pairing = store.next_reply().await?.unwrap();
        assert_eq!(pairing.receipt.unwrap().message_id, "qq_pair");
        store.delivery(pairing.id, false).await?;
        for sequence in 1..=5 {
            let reply = store.next_reply().await?.unwrap();
            let receipt = reply.receipt.unwrap();
            assert_eq!(receipt.message_id, "qq_first");
            assert_eq!(receipt.sequence, sequence);
            assert!(receipt.expires > chrono::Utc::now().timestamp());
            store.delivery(reply.id, false).await?;
        }
        assert!(store.next_reply().await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn platform_namespaces_keep_pairing_and_updates_separate() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("channels.sqlite");
        let slack = ChannelStore::open(&file, "slack:T1:A1".into())?;
        let feishu = ChannelStore::open(&file, "feishu:cli_1:tenant".into())?;
        let code = slack
            .pair("slack-token".into(), "/workspace".into())
            .await?;
        let attempt = || message(1, 10, format!("!pair {code}"));
        feishu.ingest(vec![attempt()]).await?;
        slack.ingest(vec![attempt()]).await?;
        assert!(feishu.bindings().await?.is_empty());
        assert_eq!(slack.bindings().await?[0].token, "slack-token");
        Ok(())
    }

    #[tokio::test]
    async fn new_session_rebinds_queued_prompts_and_cancel_can_pass_a_queue() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let store = ChannelStore::open(&directory.path().join("channels.sqlite"), "1".into())?;
        let code = store.pair("gateway-id".into(), "/workspace".into()).await?;
        store
            .ingest(vec![
                message(1, 10, format!("/pair {code}")),
                message(2, 10, "/new"),
                message(3, 10, "prompt"),
                message(4, 10, "!cancel\n"),
            ])
            .await?;
        let jobs = store.jobs().await?;
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].input, "/cancel");
        assert_eq!(jobs[1].input, "/new");
        store.complete(&jobs[0], "".into(), false, false).await?;
        store.complete(&jobs[1], "".into(), true, false).await?;
        let queued = store.jobs().await?;
        assert_eq!(queued[0].input, "prompt");
        assert_ne!(jobs[1].binding.session, queued[0].binding.session);
        assert_eq!(
            store.bindings().await?[0].session,
            queued[0].binding.session
        );
        Ok(())
    }
}
