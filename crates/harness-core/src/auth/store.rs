use super::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};
#[derive(Clone)]
pub(super) struct Vault {
    root: PathBuf,
}
pub(super) struct Guard {
    _lock: File,
    path: PathBuf,
}
fn fail() -> Error {
    auth_error("protected credential persistence unavailable; no credential change acknowledged")
}
fn device(profile: &str) -> bool {
    let p = profile.to_ascii_uppercase();
    matches!(p.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (p.len() == 4
            && (p.starts_with("COM") || p.starts_with("LPT"))
            && matches!(p.as_bytes()[3], b'1'..=b'9'))
}
fn plain_file(path: &Path) -> Result<()> {
    if let Ok(m) = fs::symlink_metadata(path) {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return Err(fail());
            }
        }
        if m.is_symlink() {
            return Err(fail());
        }
    }
    Ok(())
}
impl Vault {
    pub fn open(root: PathBuf, fixture: bool) -> Result<Self> {
        #[cfg(not(windows))]
        {
            let _ = (root, fixture);
            Err(auth_error(
                "Windows protected credential storage is required",
            ))
        }
        #[cfg(windows)]
        {
            if !root.is_absolute() || !crate::workspace::local_volume(&root) {
                return Err(fail());
            }
            if let Some(parent) = root.parent() {
                fs::create_dir_all(parent).map_err(|_| fail())?;
            }
            for ancestor in root.ancestors() {
                plain_file(ancestor)?;
            }
            windows::private_directory(&root)?;
            let this = Self { root };
            let guard = this.lock("host")?;
            let marker = this.root.join("mode");
            plain_file(&marker)?;
            let expected = if fixture {
                b"fixture-v1".as_slice()
            } else {
                b"production-v1".as_slice()
            };
            if marker.exists() {
                if fs::read(&marker).map_err(|_| fail())? != expected {
                    return Err(fail());
                }
            } else {
                // Never adopt an unmarked existing vault containing credentials.
                if fs::read_dir(&this.root)
                    .map_err(|_| fail())?
                    .filter_map(|e| e.ok())
                    .any(|e| e.path().extension().is_some_and(|x| x == "vault"))
                {
                    return Err(fail());
                }
                let mut f = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(marker)
                    .map_err(|_| fail())?;
                f.write_all(expected)
                    .and_then(|_| f.sync_all())
                    .map_err(|_| fail())?;
            }
            let host_path = this.root.join("host.vault");
            if !host_path.exists() {
                write(&host_path, &format!("urn:uuid:{}", uuid::Uuid::new_v4()))?;
            }
            drop(guard);
            Ok(this)
        }
    }
    pub fn host(&self) -> Result<String> {
        read(&self.root.join("host.vault"))
    }
    pub fn profiles(&self) -> Result<Vec<String>> {
        let mut profiles = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|_| fail())? {
            let p = entry.map_err(|_| fail())?.path();
            if p.extension().is_some_and(|s| s == "vault")
                && p.file_stem().is_some_and(|s| s != "host")
            {
                profiles.push(
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .ok_or_else(fail)?
                        .to_owned(),
                );
            }
        }
        profiles.sort();
        Ok(profiles)
    }
    pub fn lock(&self, profile: &str) -> Result<Guard> {
        if device(profile)
            || profile.is_empty()
            || profile.len() > 64
            || !profile
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(auth_error(
                "profile must be 1-64 ASCII letters, numbers, underscores or hyphens",
            ));
        }
        let lock = self.root.join(format!("{profile}.lock"));
        plain_file(&lock)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock)
            .map_err(|_| fail())?;
        file.try_lock().map_err(|_| Error::Busy)?;
        Ok(Guard {
            _lock: file,
            path: self.root.join(format!("{profile}.vault")),
        })
    }
    pub fn logout_version(&self, profile: &str) -> Result<Option<String>> {
        let path = self.root.join(format!("{profile}.logout"));
        if path.exists() {
            read(&path).map(Some)
        } else {
            Ok(None)
        }
    }
    pub fn stop(&self, profile: &str) -> Result<()> {
        // Validate names before building any path, even when another process owns the lock.
        if device(profile)
            || matches!(profile, "host" | "pending-login")
            || profile.is_empty()
            || profile.len() > 64
            || !profile
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(fail());
        }
        write(&self.root.join(format!("{profile}.logout")), &random())
    }
}
impl Guard {
    pub fn read(&self) -> Result<Record> {
        if self.path.exists() {
            read(&self.path)
        } else {
            Ok(Record::default())
        }
    }
    pub fn save(&self, record: &Record) -> Result<()> {
        write(&self.path, record)
    }
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    plain_file(path)?;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| fail())?
        .take(131073)
        .read_to_end(&mut bytes)
        .map_err(|_| fail())?;
    if bytes.len() > 131072 {
        return Err(fail());
    }
    #[cfg(windows)]
    {
        let plain = windows::unprotect(&bytes)?;
        serde_json::from_slice(&plain).map_err(|_| fail())
    }
    #[cfg(not(windows))]
    {
        Err(fail())
    }
}
fn write<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    plain_file(path)?;
    let bytes = serde_json::to_vec(value).map_err(|_| fail())?;
    #[cfg(windows)]
    let bytes = windows::protect(&bytes)?;
    #[cfg(not(windows))]
    {
        let _ = bytes;
        return Err(fail());
    }
    #[cfg(windows)]
    {
        if bytes.len() > 131072 {
            return Err(fail());
        }
        let mut stage =
            tempfile::NamedTempFile::new_in(path.parent().ok_or_else(fail)?).map_err(|_| fail())?;
        stage
            .write_all(&bytes)
            .and_then(|_| stage.as_file().sync_all())
            .map_err(|_| fail())?;
        windows::replace(stage.path(), path)?;
        Ok(())
    }
}
