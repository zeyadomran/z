use super::LaunchSpec;
use std::{
    ffi::OsStr,
    io,
    os::windows::ffi::OsStrExt,
    path::{Component, Path, Prefix},
};

pub(super) fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(invalid("embedded NUL"));
    }
    value.push(0);
    Ok(value)
}

pub(super) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn local_absolute(path: &Path) -> bool {
    path.is_absolute()
        && matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        && !path.components().any(|c| matches!(c, Component::ParentDir))
}

pub(super) struct Encoded {
    pub executable: Vec<u16>,
    pub command: Vec<u16>,
    pub cwd: Vec<u16>,
    pub environment: Vec<u16>,
}

pub(super) fn encode(spec: &LaunchSpec) -> io::Result<Encoded> {
    if !local_absolute(&spec.executable) || !local_absolute(&spec.cwd) {
        return Err(invalid(
            "executable and cwd must be absolute local drive paths",
        ));
    }
    if !spec
        .executable
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
    {
        return Err(invalid(
            "direct launch requires .exe; batch files require explicit M3 interpreter approval",
        ));
    }
    if !spec.executable.is_file() || !spec.cwd.is_dir() {
        return Err(invalid("executable or cwd does not exist"));
    }
    if spec.timeout.is_zero()
        || spec.timeout > std::time::Duration::from_secs(86400)
        || spec.output_limit == 0
        || spec.output_limit > 64 * 1024 * 1024
    {
        return Err(invalid(
            "timeout must be (0, 24h], output limit (0, 64 MiB]",
        ));
    }
    let executable = wide(spec.executable.as_os_str())?;
    if executable.contains(&34) {
        return Err(invalid("quote in executable"));
    }
    // argv[0] follows different CRT rules; the path cannot contain quotes.
    let mut command = vec![34];
    command.extend_from_slice(&executable[..executable.len() - 1]);
    command.push(34);
    for arg in &spec.args {
        command.extend([32, 34]);
        let arg = wide(arg)?;
        let mut slashes = 0;
        for &unit in &arg[..arg.len() - 1] {
            if unit == 92 {
                slashes += 1;
                continue;
            }
            command.extend(std::iter::repeat_n(
                92,
                if unit == 34 { slashes * 2 + 1 } else { slashes },
            ));
            command.push(unit);
            slashes = 0;
        }
        command.extend(std::iter::repeat_n(92, slashes * 2));
        command.push(34);
        if command.len() >= 32767 {
            return Err(invalid("command exceeds CreateProcessW limit"));
        }
    }
    command.push(0);
    if command.len() > 32767 {
        return Err(invalid("command exceeds CreateProcessW limit"));
    }
    let mut entries = Vec::new();
    for (name, value) in &spec.environment {
        let name = name
            .to_str()
            .ok_or_else(|| invalid("environment key must be Unicode"))?;
        if name.is_empty() || name.contains('=') {
            return Err(invalid("invalid environment key"));
        }
        let mut entry = wide(OsStr::new(name))?;
        entry.pop();
        entry.push(61);
        entry.extend(wide(value)?);
        entries.push((name.to_uppercase(), entry));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(invalid("duplicate case-insensitive environment key"));
    }
    let mut environment: Vec<_> = entries.into_iter().flat_map(|(_, value)| value).collect();
    if environment.is_empty() {
        environment.push(0);
    }
    environment.push(0);
    Ok(Encoded {
        executable,
        command,
        cwd: wide(spec.cwd.as_os_str())?,
        environment,
    })
}
