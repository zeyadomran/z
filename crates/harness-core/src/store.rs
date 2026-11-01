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

fn identity(connection: &Connection) -> Result<bool> {
    let app: i64 = connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if app == APPLICATION_ID && version == 1 {
        let contract: String = connection
            .query_row(
                "SELECT contract FROM schema_meta WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| Error::Schema)?;
        if contract != "z-m0-v1" {
            return Err(Error::Schema);
        }
        return Ok(true);
    }
    let count: i64 =
        connection.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
    if app != 0 || version != 0 || count != 0 {
        return Err(Error::Schema);
    }
    Ok(false)
}

fn open_database(path: PathBuf) -> Result<Connection> {
    let mut connection = Connection::open(path)?;
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
    if !initialized {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !identity(&transaction)? {
            transaction.execute_batch(include_str!("schema.sql"))?;
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            transaction.pragma_update(None, "user_version", 1)?;
        }
        transaction.commit()?;
    }
    Ok(connection)
}
