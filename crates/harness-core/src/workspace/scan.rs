use super::*;

#[derive(Default)]
pub(super) struct Omissions {
    count: usize,
    details: Vec<Value>,
}
impl Omissions {
    pub fn record(&mut self, path: &str, reason: &str) {
        self.count += 1;
        if self.details.len() < 20 {
            self.details.push(json!({"path":path,"reason":reason}));
        }
    }
    pub fn apply(self, value: &mut Value, more: bool) {
        value["truncated"] = json!(more || self.count > 0);
        // One entry can represent an unscanned subtree, whose file count is
        // unknown. Never imply that skipped_count counts every descendant.
        value["skipped_count"] = json!(self.count);
        value["skipped_details_truncated"] = json!(self.count > self.details.len());
        value["skipped_files"] = json!(self.details);
    }
}
#[derive(Default)]
pub(super) struct Scan {
    pub paths: Vec<String>,
    pub omissions: Omissions,
}
impl Workspace {
    pub(super) fn files(&self, input: &str) -> Result<Scan> {
        let start = self.resolve(input)?;
        let mut scan = Scan::default();
        if start.is_file() {
            self.open_verified(&start)?;
            scan.paths.push(input.into());
            return Ok(scan);
        }
        let mut queue = vec![(start, input.to_owned())];
        let mut visited = 0;
        let started = std::time::Instant::now();
        'scan: while let Some((dir, relative)) = queue.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) => {
                    scan.omissions
                        .record(&relative, &io_error(error).to_string());
                    continue;
                }
            };
            for entry in entries {
                visited += 1;
                if visited > 10000 || started.elapsed() > std::time::Duration::from_secs(5) {
                    scan.omissions
                        .record(input, "directory scan cap reached; narrow path");
                    break 'scan;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        scan.omissions
                            .record(&relative, &io_error(error).to_string());
                        continue;
                    }
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                // Policy exclusions are not inspection failures and must not
                // disclose credential names or contents through skip metadata.
                if credential(&name) || ignored(&name) || entry.path().starts_with(&self.data) {
                    continue;
                }
                let path = entry
                    .path()
                    .strip_prefix(&self.root)
                    .map_err(|_| denied())?
                    .to_string_lossy()
                    .replace('\\', "/");
                let meta = match fs::symlink_metadata(entry.path()) {
                    Ok(meta) => meta,
                    Err(error) => {
                        scan.omissions.record(&path, &io_error(error).to_string());
                        continue;
                    }
                };
                if link(&meta) {
                    scan.omissions.record(&path, "link or reparse point denied");
                    continue;
                }
                if let Err(error) = self.resolve(&path) {
                    scan.omissions.record(&path, &error.to_string());
                    continue;
                }
                if meta.is_dir() {
                    if path.split('/').count() < 16 {
                        queue.push((entry.path(), path));
                    } else {
                        scan.omissions
                            .record(&path, "directory depth cap reached; subtree omitted");
                    }
                } else if meta.is_file() {
                    match self.open_verified(&entry.path()) {
                        Ok(_) => scan.paths.push(path),
                        Err(error) => scan.omissions.record(&path, &error.to_string()),
                    }
                } else {
                    scan.omissions.record(&path, "unsupported file type");
                }
            }
        }
        scan.paths.sort();
        Ok(scan)
    }
}
