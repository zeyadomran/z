use crate::{records, store::Store, workspace::hash, *};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    path::Path,
};
pub(crate) struct WorkspaceLock {
    _file: File,
}
impl WorkspaceLock {
    /// Do not let SQLite silently recreate a missing unresolved store at its
    /// original path. Hold OS ownership through creation of a genuinely new store.
    pub async fn guard_creation(config: &Config, workspace: &str) -> Result<Option<File>> {
        let database = config.data_dir.join("harness.sqlite");
        if database.exists() {
            return Ok(None);
        }
        let identity = workspace.to_owned();
        tokio::task::spawn_blocking(move || {
            let (file, previous) = lock_record(&identity)?;
            if !previous.is_empty() && Path::new(&previous) == database && !database.exists() {
                return Err(missing_store(&previous));
            }
            Ok(Some(file))
        })
        .await
        .map_err(|_| Error::Worker)?
    }
    /// Clear the durable exclusion record only after SQLite proves the workspace
    /// has no unresolved work, while retaining the OS lock through the flush.
    pub async fn release_if_resolved(self, store: &Store, workspace: &str) -> Result<()> {
        let workspace = workspace.to_owned();
        if store
            .call(move |c| records::unresolved(c, &workspace))
            .await?
            .is_none()
        {
            tokio::task::spawn_blocking(move || {
                self._file.set_len(0).map_err(|_| Error::Busy)?;
                self._file.sync_all().map_err(|_| Error::Busy)
            })
            .await
            .map_err(|_| Error::Worker)??;
        }
        Ok(())
    }
    pub async fn acquire(config: &Config, workspace: &str) -> Result<Self> {
        let identity = workspace.to_owned();
        let database = config.data_dir.join("harness.sqlite");
        let previous_and_file = tokio::task::spawn_blocking(move || -> Result<_> {
            let (file, previous) = lock_record(&identity)?;
            Ok((file, previous, database))
        })
        .await
        .map_err(|_| Error::Worker)??;
        let (mut file, previous, database) = previous_and_file;
        if !previous.is_empty() && Path::new(&previous) != database {
            // A missing prior store cannot prove its unresolved work was resolved.
            if !Path::new(&previous).is_file() {
                return Err(missing_store(&previous));
            }
            let (store, worker) = Store::open(previous.clone().into()).await?;
            let w = workspace.to_owned();
            let busy = store.call(move |c| records::unresolved(c, &w)).await;
            drop(store);
            tokio::task::spawn_blocking(move || worker.0.join())
                .await
                .map_err(|_| Error::Worker)?
                .map_err(|_| Error::Worker)?;
            if let Some(work) = busy? {
                return Err(Error::UnresolvedWork(work));
            }
        }
        if Path::new(&previous) == database {
            return Ok(Self { _file: file });
        }
        file.rewind().map_err(|_| Error::Busy)?;
        file.set_len(0).map_err(|_| Error::Busy)?;
        file.write_all(database.to_str().ok_or(Error::Busy)?.as_bytes())
            .map_err(|_| Error::Busy)?;
        file.sync_all().map_err(|_| Error::Busy)?;
        Ok(Self { _file: file })
    }
}
fn missing_store(database: &str) -> Error {
    Error::RecoveryStoreUnavailable {
        data_dir: Path::new(database)
            .parent()
            .unwrap_or_else(|| Path::new(database))
            .to_path_buf(),
    }
}
fn lock_record(identity: &str) -> Result<(File, String)> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("z-harness")
        .join("locks");
    std::fs::create_dir_all(&base)
        .map_err(|_| Error::Configuration("cannot create workspace lock directory".into()))?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(base.join(format!("{}.lock", hash(identity.as_bytes()))))
        .map_err(|_| Error::Busy)?;
    file.try_lock().map_err(|_| Error::Busy)?;
    let mut previous = String::new();
    (&mut file)
        .take(32769)
        .read_to_string(&mut previous)
        .map_err(|_| Error::Busy)?;
    if previous.len() > 32768 {
        return Err(Error::Busy);
    }
    Ok((file, previous))
}
