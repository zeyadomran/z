use super::*;
use std::io::Write;
#[cfg(windows)]
#[allow(unsafe_code)]
#[path = "patch_windows.rs"]
mod platform;
#[cfg(not(windows))]
#[path = "patch_unsupported.rs"]
mod platform;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PatchArgs {
    pub path: String,
    pub expected_sha256: String,
    pub old_text: String,
    pub new_text: String,
}
impl PatchArgs {
    pub fn parse(text: &str) -> Result<Self> {
        let a: Self = serde_json::from_str(text).map_err(|_| denied())?;
        if text.len() > 256 * 1024
            || a.path.is_empty()
            || a.old_text.contains(['\r', '\0'])
            || a.new_text.contains(['\r', '\0'])
            || (a.expected_sha256 != "must_not_exist"
                && (a.expected_sha256.len() != 64
                    || !a
                        .expected_sha256
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())))
            || (a.expected_sha256 == "must_not_exist" && !a.old_text.is_empty())
        {
            return Err(Error::Scope(
                "invalid patch: exact LF text and whole-file expectation required".into(),
            ));
        }
        Ok(a)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Metadata {
    pub attributes: u32,
    #[serde(deserialize_with = "security_string")]
    pub security: String,
}
fn security_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    let text = String::deserialize(deserializer)?;
    // Older intents stored unused WCHAR buffer capacity after the terminator.
    // Normalize only that serialization padding, never ACEs or control flags.
    let text = text.trim_end_matches('\0');
    if text.contains('\0') {
        return Err(serde::de::Error::custom(
            "embedded NUL in security metadata",
        ));
    }
    Ok(text.to_owned())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Prepared {
    pub args: PatchArgs,
    pub target: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Vec<u8>,
    pub before_metadata: Option<Metadata>,
    pub parent_metadata: Metadata,
    pub instructions: Vec<Instruction>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Intent {
    pub prepared: Prepared,
    pub staging: PathBuf,
    pub backup: PathBuf,
    pub expected_metadata: Option<Metadata>,
}
pub(crate) enum Reconciliation {
    Applied,
    Unchanged,
    Unknown,
}

impl Workspace {
    pub(crate) fn prepare_patch(&self, arguments: &str) -> Result<Prepared> {
        let args = PatchArgs::parse(arguments)?;
        let create = args.expected_sha256 == "must_not_exist";
        let target = self.resolve_inner(&args.path, create)?;
        let instruction_path = if create {
            args.path
                .rsplit_once(['/', '\\'])
                .map(|(parent, _)| parent)
                .unwrap_or("")
        } else {
            &args.path
        };
        let instructions = self.load_instructions(instruction_path, false, &BTreeSet::new())?;
        let parent_metadata = platform::metadata(target.parent().ok_or_else(denied)?, false)?;
        let (before, before_metadata, after) = if create {
            if fs::symlink_metadata(&target).is_ok() {
                self.open_verified(&target)?;
                return Err(Error::Scope("stale: create target exists".into()));
            }
            (None, None, args.new_text.as_bytes().to_vec())
        } else {
            let (view, digest, bom, newline) = self.read(&args.path)?;
            if digest != args.expected_sha256 {
                return Err(Error::Scope("stale: whole-file hash changed".into()));
            }
            if newline == "mixed" {
                return Err(Error::Scope("mixed newlines cannot be edited".into()));
            }
            // Count overlapping matches too, avoiding ambiguous replacement.
            let positions: Vec<_> = if args.old_text.is_empty() {
                if view.is_empty() { vec![0] } else { vec![] }
            } else {
                view.char_indices()
                    .filter(|(i, _)| view[*i..].starts_with(&args.old_text))
                    .map(|(i, _)| i)
                    .take(2)
                    .collect()
            };
            if positions.len() != 1 {
                return Err(Error::Scope("old_text must match exactly once".into()));
            }
            let before = self.patch_bytes(&args.path)?;
            if hash(&before) != digest {
                return Err(Error::Scope("stale: file changed during preview".into()));
            }
            let mut edited = view.clone();
            edited.replace_range(
                positions[0]..positions[0] + args.old_text.len(),
                &args.new_text,
            );
            if newline == "crlf" {
                edited = edited.replace('\n', "\r\n");
            }
            if bom {
                edited.insert(0, '\u{feff}');
            }
            let metadata = platform::metadata(&target, true)?;
            (Some(before), Some(metadata), edited.into_bytes())
        };
        if after.len() > 1024 * 1024 {
            return Err(Error::Scope("patch exceeds 1 MiB".into()));
        }
        if before.as_ref() == Some(&after) {
            return Err(Error::Scope("patch has no change".into()));
        }
        Ok(Prepared {
            args,
            target,
            before,
            after,
            before_metadata,
            parent_metadata,
            instructions,
        })
    }
    fn patch_bytes(&self, relative: &str) -> Result<Vec<u8>> {
        self.raw_snapshot(relative)
    }
    pub(crate) fn reconcile_patch(&self, intent: &Intent) -> Reconciliation {
        let p = &intent.prepared;
        let now = self.patch_bytes(&p.args.path);
        if let Ok(bytes) = &now {
            let metadata = platform::metadata(&p.target, true).ok();
            if bytes == &p.after && metadata.is_some() && metadata == intent.expected_metadata {
                return Reconciliation::Applied;
            }
            if Some(bytes) == p.before.as_ref() && metadata == p.before_metadata {
                return Reconciliation::Unchanged;
            }
        } else if p.before.is_none()
            && self.resolve_inner(&p.args.path, true).is_ok()
            && matches!(fs::symlink_metadata(&p.target), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        {
            return Reconciliation::Unchanged;
        }
        Reconciliation::Unknown
    }
    pub(crate) fn stage_patch(&self, intent: &mut Intent) -> Result<()> {
        self.revalidate(&intent.prepared)?;
        let mut stage = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&intent.staging)
            .map_err(io_error)?;
        if let Some(meta) = &intent.prepared.before_metadata {
            platform::secure_stage(&intent.staging, meta)?;
        }
        stage.write_all(&intent.prepared.after).map_err(io_error)?;
        stage.sync_all().map_err(io_error)?;
        drop(stage);
        if let Some(meta) = &intent.prepared.before_metadata {
            platform::stage_attributes(&intent.staging, meta)?;
            intent.expected_metadata = Some(meta.clone());
        } else {
            intent.expected_metadata = Some(platform::metadata(&intent.staging, true)?);
        }
        Ok(())
    }
    pub(crate) fn revalidate(&self, p: &Prepared) -> Result<()> {
        if self.prepare_patch(&serde_json::to_string(&p.args)?)? != *p {
            return Err(Error::Scope("stale: approval binding changed".into()));
        }
        Ok(())
    }
    pub(crate) fn replace_patch(&self, intent: &Intent) -> Result<()> {
        self.revalidate(&intent.prepared)?;
        // Names are unique, recorded before staging, and never overwritten.
        if intent.backup.try_exists().map_err(io_error)? {
            return Err(denied());
        }
        platform::replace(intent, || self.revalidate(&intent.prepared))?;
        if !matches!(self.reconcile_patch(intent), Reconciliation::Applied) {
            return Err(Error::Scope(
                "replacement content or metadata is uncertain".into(),
            ));
        }
        Ok(())
    }
}
