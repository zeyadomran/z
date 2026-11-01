#[cfg(windows)]
pub fn acl(path: &std::path::Path, customize: bool) -> String {
    let script = if customize {
        "$a=Get-Acl -LiteralPath $env:OWNED_PATCH_FILE; $a.SetAccessRuleProtection($true,$true); $sid=[System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-545'); $r=[System.Security.AccessControl.FileSystemAccessRule]::new($sid,'Read','Allow'); $a.AddAccessRule($r); Set-Acl -LiteralPath $env:OWNED_PATCH_FILE -AclObject $a; [System.IO.File]::SetAttributes($env:OWNED_PATCH_FILE, [System.IO.FileAttributes]::Hidden -bor [System.IO.FileAttributes]::Archive -bor [System.IO.FileAttributes]::NotContentIndexed); (Get-Acl -LiteralPath $env:OWNED_PATCH_FILE).Sddl"
    } else {
        "(Get-Acl -LiteralPath $env:OWNED_PATCH_FILE).Sddl"
    };
    let result = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("OWNED_PATCH_FILE", path)
        .output()
        .unwrap();
    assert!(result.status.success(), "owned ACL qualification failed");
    String::from_utf8(result.stdout).unwrap().trim().to_string()
}
