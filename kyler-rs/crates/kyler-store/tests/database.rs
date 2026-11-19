use std::sync::{Arc, Barrier};

use anyhow::Result;
use kyler_store::Database;

#[test]
fn policy_migrations_rollback_and_future_versions_are_preserved() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("nested/app.sqlite3");
    let database = Database::open(&path)?;
    database.with(|connection| {
        assert_eq!(
            connection.pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))?,
            "wal"
        );
        assert_eq!(
            connection.pragma_query_value(None, "foreign_keys", |r| r.get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            connection.pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))?,
            2
        );
        assert_eq!(
            connection.pragma_query_value(None, "busy_timeout", |r| r.get::<_, i64>(0))?,
            5000
        );
        Ok(())
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(
        database
            .migrate(1, |transaction, _| {
                transaction.execute_batch("CREATE TABLE partial(id INTEGER);")?;
                anyhow::bail!("injected migration failure")
            })
            .is_err()
    );
    database.with(|connection| {
        assert!(connection.prepare("SELECT * FROM partial").is_err());
        assert_eq!(
            connection.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))?,
            0
        );
        Ok(())
    })?;
    database.migrate(1, |transaction, current| {
        assert_eq!(current, 0);
        transaction
            .execute_batch("CREATE TABLE durable(id INTEGER); INSERT INTO durable VALUES(7);")?;
        Ok(())
    })?;
    assert!(
        database
            .migrate(0, |_, _| panic!("future schema must not migrate"))
            .is_err()
    );
    let reopened = Database::open(&path)?;
    reopened.migrate(1, |_, _| panic!("already migrated"))?;
    reopened.with(|connection| {
        assert_eq!(
            connection.query_row("SELECT id FROM durable", [], |r| r.get::<_, i64>(0))?,
            7
        );
        Ok(())
    })
}

#[test]
fn concurrent_connections_apply_schema_once() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("app.sqlite3");
    let databases = [Database::open(&path)?, Database::open(&path)?];
    let gate = Arc::new(Barrier::new(2));
    let workers = databases
        .into_iter()
        .map(|database| {
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                database.migrate(1, |transaction, _| {
                    transaction.execute_batch(
                        "CREATE TABLE migrated(id INTEGER); INSERT INTO migrated VALUES(1);",
                    )?;
                    Ok(())
                })
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap()?;
    }
    Database::open(&path)?.with(|connection| {
        assert_eq!(
            connection.query_row("SELECT count(*) FROM migrated", [], |r| r.get::<_, i64>(0))?,
            1
        );
        Ok(())
    })
}

#[tokio::test]
async fn cancelled_async_caller_keeps_its_operation_and_lock_alive() -> Result<()> {
    let root = tempfile::tempdir()?;
    let database = Database::open(&root.path().join("app.sqlite3"))?;
    database.with(|connection| {
        connection.execute_batch("CREATE TABLE writes(value INTEGER);")?;
        Ok(())
    })?;
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (release, wait_release) = std::sync::mpsc::channel();
    let writer = database.clone();
    let task = tokio::spawn(async move {
        writer
            .with_async(move |connection| {
                let transaction = connection.transaction()?;
                transaction.execute("INSERT INTO writes VALUES(1)", [])?;
                started.send(()).unwrap();
                wait_release.recv()?;
                transaction.commit()?;
                Ok(())
            })
            .await
    });
    wait_started.await?;
    task.abort();
    release.send(())?;
    database
        .with_async(|connection| {
            assert_eq!(
                connection.query_row("SELECT value FROM writes", [], |r| r.get::<_, i64>(0))?,
                1
            );
            Ok(())
        })
        .await
}
