use crate::{Error, Result};
use rusqlite::{Connection, TransactionBehavior};
use std::{path::PathBuf, thread, time::Duration};
use tokio::sync::{mpsc, oneshot};

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

#[derive(Clone)]
pub(crate) struct Store(mpsc::Sender<Job>);

pub(crate) struct StoreWorker(pub thread::JoinHandle<()>);

impl Store {
    pub async fn open(path: PathBuf) -> Result<(Self, StoreWorker)> {
        let (sender, mut receiver) = mpsc::channel::<Job>(16);
        let (ready, result) = oneshot::channel();
        let worker = thread::Builder::new()
            .name("harness-sqlite".into())
            .spawn(move || {
                let opened = open_database(path);
                match opened {
                    Ok(mut connection) => {
                        if ready.send(Ok(())).is_err() {
                            return;
                        }
                        while let Some(job) = receiver.blocking_recv() {
                            job(&mut connection);
                        }
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                }
            })
            .map_err(|_| Error::Worker)?;
        match result.await.map_err(|_| Error::Worker)? {
            Ok(()) => Ok((Self(sender), StoreWorker(worker))),
            Err(error) => {
                tokio::task::spawn_blocking(move || worker.join())
                    .await
                    .map_err(|_| Error::Worker)?
                    .map_err(|_| Error::Worker)?;
                Err(error)
            }
        }
    }

    pub async fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (sender, receiver) = oneshot::channel();
        self.0
            .send(Box::new(move |connection| {
                let _ = sender.send(operation(connection));
            }))
            .await
            .map_err(|_| Error::Worker)?;
        receiver.await.map_err(|_| Error::Worker)?
    }
}

const APPLICATION_ID: i64 = 0x5a484152;

fn identity(connection: &Connection) -> Result<i64> {
    let app: i64 = connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if app == APPLICATION_ID && (version == 1 || version == 2) {
        let contract: String = connection
            .query_row(
                "SELECT contract FROM schema_meta WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| Error::Schema)?;
        if contract != if version == 1 { "z-m0-v1" } else { "z-m1a-v2" } {
            return Err(Error::Schema);
        }
        return Ok(version);
    }
    let count: i64 =
        connection.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
    if app != 0 || version != 0 || count != 0 {
        return Err(Error::Schema);
    }
    Ok(0)
}

fn open_database(path: PathBuf) -> Result<Connection> {
    let mut connection = Connection::open(&path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    // Unknown schemas must be rejected before even changing journal mode.
    let initialized = identity(&connection)?;
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    connection.pragma_update(None, "synchronous", "EXTRA")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    let journal: String = connection.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    let sync: i64 = connection.pragma_query_value(None, "synchronous", |r| r.get(0))?;
    let fk: i64 = connection.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
    let busy: i64 = connection.pragma_query_value(None, "busy_timeout", |r| r.get(0))?;
    if journal != "delete" || sync != 3 || fk != 1 || busy != 5000 {
        return Err(Error::Persistence(
            "SQLite durability settings were not accepted".into(),
        ));
    }
    if initialized == 0 {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if identity(&transaction)? == 0 {
            transaction.execute_batch(include_str!("schema.sql"))?;
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            transaction.pragma_update(None, "user_version", 1)?;
        }
        transaction.commit()?;
    }
    migrate(&mut connection, &path)?;
    Ok(connection)
}

fn migrate(connection: &mut Connection, path: &std::path::Path) -> Result<()> {
    if identity(connection)? == 2 {
        return Ok(());
    }
    let migration_lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("migration-lock"))
        .map_err(|_| Error::Persistence("cannot open migration lock".into()))?;
    migration_lock.try_lock().map_err(|_| Error::Busy)?;
    if identity(connection)? == 2 {
        return Ok(());
    }
    let mut backup = path.with_extension("v1.backup.sqlite");
    // Every migration attempt retains a fresh consistent snapshot. Never reuse
    // a stale backup after a failed attempt or overwrite a previous artifact.
    if backup.exists() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::Worker)?
            .as_nanos();
        backup = path.with_extension(format!("v1.backup.{stamp}.sqlite"));
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .map_err(|_| Error::Persistence("cannot reserve migration backup".into()))?;
    connection.backup("main", &backup, None::<fn(rusqlite::backup::Progress)>)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if identity(&tx)? != 1 {
        return Err(Error::Schema);
    }
    tx.execute_batch(include_str!("migration_v2.sql"))?;
    let rows = {
        let mut q = tx.prepare("SELECT turn_id,epoch_id,role,content FROM messages ORDER BY id")?;
        q.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (turn, epoch, role, text) in rows {
        let message = if role == "user" {
            crate::ProtocolMessage::User(text)
        } else {
            crate::ProtocolMessage::Assistant {
                text,
                calls: vec![],
            }
        };
        tx.execute(
            "INSERT INTO protocol_messages(turn_id,epoch_id,payload) VALUES(?1,?2,?3)",
            rusqlite::params![turn, epoch, serde_json::to_string(&message)?],
        )?;
    }
    tx.commit()?;
    Ok(())
}
