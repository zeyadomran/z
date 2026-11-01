#[cfg(windows)]
pub fn acl(path: &std::path::Path, customize: bool) -> String {
    let script = if customize {
        "$a=Get-Acl -LiteralPath $env:OWNED_PATCH_FILE; $a.SetAccessRuleProtection($true,$true); $sid=[System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-545'); $r=[System.Security.AccessControl.FileSystemAccessRule]::new($sid,'Read','Allow'); $a.AddAccessRule($r); Set-Acl -LiteralPath $env:OWNED_PATCH_FILE -AclObject $a; [System.IO.File]::SetAttributes($env:OWNED_PATCH_FILE, [System.IO.FileAttributes]::Hidden -bor [System.IO.FileAttributes]::Archive -bor [System.IO.FileAttributes]::NotContentIndexed); (Get-Acl -LiteralPath $env:OWNED_PATCH_FILE).Sddl"
    } else {
        "(Get-Acl -LiteralPath $env:OWNED_PATCH_FILE).Sddl"
    };
    run(path, script)
}

// Reproduce the hosted runner's legacy parent: inheritable explicit ACEs, no
// P/AI flags. Use the current token SID so this needs no administrator rights.
#[cfg(windows)]
pub fn legacy_parent(path: &std::path::Path) -> String {
    run(path, &format!("{LEGACY_API}\n{LEGACY_PARENT}"))
}

#[cfg(windows)]
pub fn raw_acl(path: &std::path::Path) -> String {
    run(
        path,
        &format!("{LEGACY_API}\n[OwnedPatchAcl]::Read($env:OWNED_PATCH_FILE)"),
    )
}

// Materialize the native replacement/restoration gap with real Windows APIs.
// The recovery owner controls this external boundary after killing its child.
#[cfg(windows)]
#[allow(dead_code)] // This helper is only used by the recovery test binary.
pub fn replace_without_restore(path: &std::path::Path) {
    run(
        path,
        &format!(
            "{LEGACY_API}\n{}",
            r#"
$file=$env:OWNED_PATCH_FILE
$stage="$file.owned-merge-stage"
$backup="$file.owned-merge-backup"
if ([System.IO.File]::Exists($stage) -or [System.IO.File]::Exists($backup)) { throw 'Fixture collision' }
$original=[OwnedPatchAcl]::Read($file)
[System.IO.File]::WriteAllBytes($stage,[System.IO.File]::ReadAllBytes($file))
Set-OwnedAcl $stage $original
if ([OwnedPatchAcl]::Read($stage) -ne $original) { throw 'Stage must exactly match before replacement' }
if (![OwnedPatchAcl]::ReplaceFile($file,$stage,$backup,0,[IntPtr]::Zero,[IntPtr]::Zero)) { throw [System.ComponentModel.Win32Exception]::new() }
$actual=[OwnedPatchAcl]::Read($file)
# Windows builds may retain duplicate explicit ACEs or convert them to inherited
# ACEs. Both differ from the approved legacy descriptor and must remain unknown.
if ($actual -eq $original -or !$actual.Contains('D:AI') -or !$actual.Contains('(A;ID;FA;;;SY)')) { throw "Native merge not reproduced: $actual" }
"#
        ),
    );
}

#[cfg(windows)]
const LEGACY_API: &str = r#"
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class OwnedPatchAcl {
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern bool ReplaceFile(string target, string stage, string backup, uint flags, IntPtr exclude, IntPtr reserved);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern bool SetFileSecurity(string path, uint info, byte[] descriptor);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool GetFileSecurity(string path, uint info, byte[] descriptor, uint size, out uint needed);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool ConvertSecurityDescriptorToStringSecurityDescriptor(byte[] descriptor, uint revision, uint info, out IntPtr text, out uint length);
    [DllImport("kernel32.dll")]
    static extern IntPtr LocalFree(IntPtr memory);
    public static string Read(string path) {
        uint needed;
        GetFileSecurity(path,7,null,0,out needed);
        var bytes=new byte[needed];
        if (!GetFileSecurity(path,7,bytes,needed,out needed)) throw new System.ComponentModel.Win32Exception();
        IntPtr text; uint length;
        if (!ConvertSecurityDescriptorToStringSecurityDescriptor(bytes,1,7,out text,out length)) throw new System.ComponentModel.Win32Exception();
        try { return Marshal.PtrToStringUni(text); } finally { LocalFree(text); }
    }
}
'@
$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$sddl="D:(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;$sid)"
function Set-OwnedAcl($path,$s) {
    $sd=[System.Security.AccessControl.RawSecurityDescriptor]::new($s)
    $bytes=[byte[]]::new($sd.BinaryLength)
    $sd.GetBinaryForm($bytes,0)
    if (![OwnedPatchAcl]::SetFileSecurity($path,4,$bytes)) { throw [System.ComponentModel.Win32Exception]::new() }
}
"#;

#[cfg(windows)]
const LEGACY_PARENT: &str = r#"
# Bootstrap only the owned fixture root to the legacy model. No parent outside
# this temporary root is changed; children then start without the AI control bit.
Set-OwnedAcl ([System.IO.Path]::GetDirectoryName($env:OWNED_PATCH_FILE)) $sddl.Replace('D:','D:P').Replace(';OICI;',';;')
[System.IO.Directory]::CreateDirectory($env:OWNED_PATCH_FILE) | Out-Null
Set-OwnedAcl $env:OWNED_PATCH_FILE $sddl
$actual=[OwnedPatchAcl]::Read($env:OWNED_PATCH_FILE)
$expectedDescriptor=[System.Security.AccessControl.RawSecurityDescriptor]::new($sddl)
$actualDescriptor=[System.Security.AccessControl.RawSecurityDescriptor]::new($actual)
$expectedAcl=[byte[]]::new($expectedDescriptor.DiscretionaryAcl.BinaryLength)
$actualAcl=[byte[]]::new($actualDescriptor.DiscretionaryAcl.BinaryLength)
$expectedDescriptor.DiscretionaryAcl.GetBinaryForm($expectedAcl,0)
$actualDescriptor.DiscretionaryAcl.GetBinaryForm($actualAcl,0)
# Compare actual SIDs/ACEs, not SDDL aliases (the runner's user can render as LA).
# 0x1500 covers DACL protection and auto-inheritance/request control bits.
if (($actualDescriptor.ControlFlags -band 0x1500) -ne 0 -or
    [Convert]::ToBase64String($expectedAcl) -cne [Convert]::ToBase64String($actualAcl)) {
    throw "Legacy parent not established: $actual"
}
$actual
"#;

#[cfg(windows)]
fn run(path: &std::path::Path, script: &str) -> String {
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
        "owned ACL qualification failed (status={}): {}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stderr),
        String::from_utf8_lossy(&result.stdout),
    );
    String::from_utf8(result.stdout).unwrap().trim().to_string()
}
