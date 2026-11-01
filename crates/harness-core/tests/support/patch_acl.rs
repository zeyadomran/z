#[cfg(windows)]
pub fn acl(path: &std::path::Path, customize: bool) -> String {
    let script = if customize {
        "$a=Get-Acl -LiteralPath $env:OWNED_PATCH_FILE; $a.SetAccessRuleProtection($true,$true); $sid=[System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-545'); $r=[System.Security.AccessControl.FileSystemAccessRule]::new($sid,'Read','Allow'); $a.AddAccessRule($r); Set-Acl -LiteralPath $env:OWNED_PATCH_FILE -AclObject $a; [System.IO.File]::SetAttributes($env:OWNED_PATCH_FILE, [System.IO.FileAttributes]::Hidden -bor [System.IO.FileAttributes]::Archive -bor [System.IO.FileAttributes]::NotContentIndexed); (Get-Acl -LiteralPath $env:OWNED_PATCH_FILE).Sddl"
    } else {
        "(Get-Acl -LiteralPath $env:OWNED_PATCH_FILE).Sddl"
    };
    let script = format!("$ErrorActionPreference='Stop'; {script}");
    let result = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        // Cargo can inherit PowerShell 7 module paths on CI. Let Windows
        // PowerShell rebuild its own paths instead of loading Core modules.
        .env_remove("PSModulePath")
        .env("OWNED_PATCH_FILE", path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "owned ACL qualification failed (customize={customize}, status={}): {}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stderr),
        String::from_utf8_lossy(&result.stdout),
    );
    String::from_utf8(result.stdout).unwrap().trim().to_string()
}
