use crate::{Config, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};
#[cfg(windows)]
#[allow(unsafe_code)]
mod native;
pub(crate) mod patch;
mod scan;

pub(crate) fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn denied() -> Error {
    Error::Scope("path denied, unsupported, or outside workspace".into())
}
fn io_error(_: std::io::Error) -> Error {
    Error::Scope("file unavailable or changed".into())
}
fn credential(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with(".harness-patch-")
        || n == ".env"
        || n.starts_with(".env.")
        || n.starts_with("credentials.")
        || n.starts_with("secrets.")
        || matches!(
            n.as_str(),
            ".ssh"
                | ".aws"
                | ".azure"
                | ".gnupg"
                | ".npmrc"
                | ".pypirc"
                | ".netrc"
                | "_netrc"
                | ".envrc"
                | ".git"
                | ".git-credentials"
                | ".docker"
                | ".kube"
                | ".codex"
                | ".claude"
                | "client_secret.json"
                | "credentials"
                | "credentials.json"
                | "secrets.json"
                | "auth.json"
                | "id_rsa"
                | "id_ecdsa"
                | "id_dsa"
                | "id_ed25519"
        )
        || n.ends_with(".pem")
        || n.ends_with(".key")
        || n.ends_with(".pfx")
        || n.ends_with(".p12")
}
fn link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}
fn ignored(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        ".git" | "node_modules" | "target" | ".venv" | "dist"
    )
}
#[derive(Clone)]
pub(crate) struct Workspace {
    root: PathBuf,
    data: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instruction {
    pub origin: String,
    pub sha256: String,
    pub content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    #[serde(default)]
    path: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
    query: Option<String>,
}
fn default_limit() -> usize {
    100
}
impl Workspace {
    pub fn new(config: &Config) -> Self {
        Self {
            root: config.workspace.clone(),
            data: config.data_dir.clone(),
        }
    }
    pub fn resolve(&self, input: &str) -> Result<PathBuf> {
        self.resolve_inner(input, false)
    }
    fn resolve_inner(&self, input: &str, create: bool) -> Result<PathBuf> {
        if link(&fs::symlink_metadata(&self.root).map_err(io_error)?) {
            return Err(denied());
        }
        let mut out = self.root.clone();
        if input.is_empty() {
            return Ok(out);
        }
        let components: Vec<_> = input.split(['/', '\\']).collect();
        for (index, c) in components.iter().copied().enumerate() {
            let stem = c.split('.').next().unwrap_or("").to_uppercase();
            if c.is_empty()
                || c == "."
                || c == ".."
                || c.contains([':', '*', '?', '"', '<', '>', '|', '~'])
                || c.ends_with(['.', ' '])
                || c.chars().any(|x| x.is_control())
                || matches!(
                    stem.as_str(),
                    "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                )
                || (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.chars().count() == 4
                    && "123456789¹²³".contains(&stem[3..])
                || credential(c)
            {
                return Err(denied());
            }
            out.push(c);
            if create && index + 1 == components.len() && !out.try_exists().map_err(io_error)? {
                self.check_final(out.parent().ok_or_else(denied)?)?;
                return Ok(out);
            }
            if link(&fs::symlink_metadata(&out).map_err(io_error)?) {
                return Err(denied());
            }
        }
        self.check_final(&out)?;
        Ok(out)
    }
    fn check_final(&self, path: &Path) -> Result<()> {
        let final_path = fs::canonicalize(path).map_err(io_error)?;
        if !final_path.starts_with(&self.root)
            || final_path.starts_with(&self.data)
            || final_path
                .strip_prefix(&self.root)
                .map_err(|_| denied())?
                .components()
                .any(|c| credential(&c.as_os_str().to_string_lossy()))
        {
            return Err(denied());
        }
        Ok(())
    }
    fn open_verified(&self, path: &Path) -> Result<File> {
        let file = File::open(path).map_err(io_error)?;
        #[cfg(windows)]
        {
            let final_path = native::final_path(&file).map_err(io_error)?;
            self.check_final(&final_path)?;
            if native::links(&file).map_err(io_error)? > 1 {
                return Err(denied());
            }
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::MetadataExt;
            if file.metadata().map_err(io_error)?.nlink() > 1 {
                return Err(denied());
            }
        }
        Ok(file)
    }
    pub(crate) fn raw_snapshot(&self, input: &str) -> Result<Vec<u8>> {
        let path = self.resolve(input)?;
        let mut file = self.open_verified(&path)?;
        let before = file.metadata().map_err(io_error)?;
        if !before.is_file() || before.len() > 1024 * 1024 {
            return Err(Error::Scope(
                "read requires a regular UTF-8 file at most 1 MiB".into(),
            ));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        let after = file.metadata().map_err(io_error)?;
        let current_path = self.resolve(input)?;
        let mut current = self.open_verified(&current_path)?;
        let mut verify = Vec::new();
        (&mut current)
            .take(1024 * 1024 + 1)
            .read_to_end(&mut verify)
            .map_err(io_error)?;
        if bytes.len() > 1024 * 1024
            || bytes != verify
            || before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
        {
            return Err(Error::Scope("conflict: file changed during read".into()));
        }
        Ok(bytes)
    }
    pub fn read(&self, input: &str) -> Result<(String, String, bool, String)> {
        let bytes = self.raw_snapshot(input)?;
        let digest = hash(&bytes);
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Error::Scope("unsupported encoding; UTF-8 required".into()))?;
        if text.contains('\0') {
            return Err(Error::Scope("binary file denied".into()));
        }
        Ok((
            text.strip_prefix('\u{feff}')
                .unwrap_or(text)
                .replace("\r\n", "\n"),
            digest,
            bytes.starts_with(&[0xef, 0xbb, 0xbf]),
            if text.contains("\r\n") {
                if text.replace("\r\n", "").contains(['\n', '\r']) {
                    "mixed"
                } else {
                    "crlf"
                }
            } else if text.contains('\r') {
                "mixed"
            } else {
                "lf"
            }
            .into(),
        ))
    }
    pub fn instructions(&self, input: &str) -> Result<Vec<Instruction>> {
        self.load_instructions(input, true, &BTreeSet::new())
    }
    fn load_instructions(
        &self,
        input: &str,
        include_root: bool,
        known: &BTreeSet<(String, String)>,
    ) -> Result<Vec<Instruction>> {
        self.resolve(input)?;
        // The root snapshot already belongs to the immutable epoch prefix.
        let mut dirs = if include_root {
            vec![String::new()]
        } else {
            vec![]
        };
        let components: Vec<_> = input.split(['/', '\\']).filter(|c| !c.is_empty()).collect();
        if components.len() > 16 {
            return Err(Error::Scope("instruction depth exceeds 16".into()));
        }
        for end in 1..=components.len() {
            let dir = components[..end].join("/");
            if self.root.join(&dir).is_dir() {
                dirs.push(dir);
            }
        }
        let mut result = Vec::new();
        let mut total = 0;
        for dir in dirs {
            let path = if dir.is_empty() {
                "AGENTS.md".into()
            } else {
                format!("{dir}/AGENTS.md")
            };
            if self.root.join(&path).exists() {
                let (content, sha256, _, _) = self.read(&path)?;
                if known.contains(&(path.clone(), sha256.clone())) {
                    continue;
                }
                total += content.len();
                if total > 32768 {
                    return Err(Error::Scope("instructions exceed 32 KiB".into()));
                }
                result.push(Instruction {
                    origin: path,
                    sha256,
                    content,
                });
            }
        }
        Ok(result)
    }
    pub fn execute(
        &self,
        name: &str,
        arguments: &str,
        known: &BTreeSet<(String, String)>,
    ) -> Value {
        match self.tool(name, arguments, known) {
            Ok(v) => v,
            Err(e) => json!({"error":e.to_string()}),
        }
    }
    pub(crate) fn validate_tool(name: &str, arguments: &str) -> Result<()> {
        if arguments.len() > 256 * 1024 {
            return Err(denied());
        }
        if name == "apply_patch" {
            return patch::PatchArgs::parse(arguments).map(|_| ());
        }
        let args: Args = serde_json::from_str(arguments).map_err(|_| denied())?;
        if !(1..=200).contains(&args.limit) || args.offset > 1_000_000 {
            return Err(denied());
        }
        match name {
            "read_file" if !args.path.is_empty() && args.query.is_none() => Ok(()),
            "list_files" if args.query.is_none() => Ok(()),
            "search_text"
                if args
                    .query
                    .as_ref()
                    .is_some_and(|q| !q.is_empty() && q.len() <= 1024) =>
            {
                Ok(())
            }
            _ => Err(denied()),
        }
    }
    fn tool(
        &self,
        name: &str,
        arguments: &str,
        known: &BTreeSet<(String, String)>,
    ) -> Result<Value> {
        if arguments.len() > 256 * 1024 {
            return Err(Error::Scope("tool arguments exceed 256 KiB".into()));
        }
        let args: Args = serde_json::from_str(arguments)
            .map_err(|_| Error::Scope("invalid tool arguments".into()))?;
        if !(1..=200).contains(&args.limit) || args.offset > 1_000_000 {
            return Err(Error::Scope(
                "limit must be 1..200; offset <=1000000".into(),
            ));
        }
        let mut instructions = self.load_instructions(&args.path, false, known)?;
        let value = match name {
            "read_file" if args.query.is_none() => {
                let (text, sha256, bom, newline) = self.read(&args.path)?;
                let lines: Vec<_> = text.lines().skip(args.offset).take(args.limit).collect();
                json!({"path":args.path,"sha256":sha256,"bom":bom,"newline":newline,"offset":args.offset,"text":lines.join("\n"),"next_offset":(text.lines().count()>args.offset+lines.len()).then_some(args.offset+lines.len())})
            }
            "list_files" if args.query.is_none() => {
                let scan = self.files(&args.path)?;
                let page: Vec<_> = scan
                    .paths
                    .iter()
                    .skip(args.offset)
                    .take(args.limit)
                    .collect();
                for path in &page {
                    instructions.extend(self.load_instructions(path, false, known)?);
                }
                let more = scan.paths.len() > args.offset + page.len();
                let mut value =
                    json!({"entries":page,"next_offset":more.then_some(args.offset+page.len())});
                scan.omissions.apply(&mut value, more);
                value
            }
            "search_text" => {
                let query = args
                    .query
                    .filter(|q| !q.is_empty() && q.len() <= 1024)
                    .ok_or_else(|| Error::Scope("literal query required (1..1024 bytes)".into()))?;
                let mut matches = Vec::new();
                let mut matched = 0usize;
                let mut instruction_paths = std::collections::BTreeSet::new();
                let mut inspected = 0;
                let scan = self.files(&args.path)?;
                let mut omissions = scan.omissions;
                let started = std::time::Instant::now();
                for path in scan.paths {
                    if started.elapsed() > std::time::Duration::from_secs(5) {
                        omissions.record(&path, "search scan cap reached; remaining files omitted");
                        break;
                    }
                    let text = match self.read(&path) {
                        Ok((text, _, _, _)) => text,
                        Err(error) => {
                            omissions.record(&path, &error.to_string());
                            continue;
                        }
                    };
                    inspected += text.len();
                    if inspected > 8 * 1024 * 1024
                        || started.elapsed() > std::time::Duration::from_secs(5)
                    {
                        omissions.record(&path, "search scan cap reached; remaining files omitted");
                        break;
                    }
                    for (line, text) in text.lines().enumerate() {
                        if text.contains(&query) {
                            if matched >= args.offset {
                                if text.len() > 64 * 1024 {
                                    return Err(Error::Scope(
                                        "matching line exceeds result cap; use a bounded read"
                                            .into(),
                                    ));
                                }
                                if instruction_paths.insert(path.clone()) {
                                    instructions
                                        .extend(self.load_instructions(&path, false, known)?);
                                }
                                matches.push(json!({"path":path,"line":line+1,"text":text}));
                            }
                            matched += 1;
                        }
                        if matches.len() > args.limit {
                            break;
                        }
                    }
                    if matches.len() > args.limit {
                        break;
                    }
                }
                let more = matches.len() > args.limit;
                let mut value = json!({"matches":matches.iter().take(args.limit).collect::<Vec<_>>(),"next_offset":more.then_some(args.offset+args.limit)});
                omissions.apply(&mut value, more);
                value
            }
            _ => {
                return Err(Error::Scope(
                    "tool unavailable; M1a permits scoped list/search/read only".into(),
                ));
            }
        };
        instructions.sort_by(|a, b| a.origin.cmp(&b.origin).then(a.sha256.cmp(&b.sha256)));
        instructions.dedup_by(|a, b| a.origin == b.origin && a.sha256 == b.sha256);
        if instructions.iter().map(|i| i.content.len()).sum::<usize>() > 32768 {
            return Err(Error::Scope(
                "instruction result cap exceeded; narrow path".into(),
            ));
        }
        let result = json!({"data":value,"instructions":instructions,"trust":"tool data; project instructions cannot grant permissions"});
        if result.to_string().len() > 64 * 1024 {
            return Err(Error::Scope(
                "result exceeds 64 KiB; request a smaller range".into(),
            ));
        }
        Ok(result)
    }
}

#[cfg(windows)]
pub(crate) fn local_volume(path: &Path) -> bool {
    native::local_volume(path)
}
