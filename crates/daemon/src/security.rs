//! Filesystem trust checks precede reading credentials or publishing invitations.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Component, Path},
};

pub fn absolute(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "configuration paths must be absolute");
    ensure!(
        !path
            .components()
            .any(|part| matches!(part, Component::ParentDir)),
        "configuration paths must not contain parent traversal"
    );
    Ok(())
}

/// Reject links/reparse points and directories writable by another principal.
/// Sticky system temporary directories may be ancestors, never the private leaf.
pub fn ancestors(path: &Path) -> Result<()> {
    absolute(path)?;
    let mut current = std::path::PathBuf::new();
    for component in path.components() {
        current.push(component);
        #[cfg(windows)]
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        let metadata = fs::symlink_metadata(&current).context("cannot inspect path component")?;
        ensure!(metadata.is_dir(), "path component is not a real directory");
        reject_link(&metadata)?;
        #[cfg(windows)]
        windows_acl(&current, Access::Ancestor)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            trusted_owner(&metadata)?;
            ensure!(
                metadata.mode() & 0o022 == 0 || metadata.mode() & 0o1000 != 0,
                "path ancestor is writable by another user"
            );
        }
    }
    #[cfg(windows)]
    windows_acl(path, Access::Parent)?;
    Ok(())
}

pub fn private(path: &Path, directory: bool) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path).context("cannot inspect private path")?;
    reject_link(&metadata)?;
    ensure!(
        if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        },
        "private path has the wrong type"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        trusted_owner(&metadata)?;
        ensure!(
            metadata.mode() & 0o077 == 0,
            "private path grants group or other access"
        );
        if !directory {
            ensure!(
                metadata.nlink() == 1,
                "private file must not have hard links"
            );
        }
    }
    #[cfg(windows)]
    windows_acl(path, Access::Private)?;
    Ok(metadata)
}

fn reject_link(metadata: &fs::Metadata) -> Result<()> {
    ensure!(
        !metadata.file_type().is_symlink(),
        "symbolic links are not allowed"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "reparse points are not allowed"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn trusted_owner(metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let owner = metadata.uid();
    ensure!(
        owner == 0 || owner == rustix::process::geteuid().as_raw(),
        "path owner must be the service account or root"
    );
    Ok(())
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum Access {
    Ancestor,
    Parent,
    Private,
}

#[cfg(windows)]
fn windows_acl(path: &Path, access: Access) -> Result<()> {
    // Use the OS ACL implementation without unsafe FFI. Fail closed when
    // PowerShell/ACL inspection is unavailable. No config contents enter argv.
    let script = r#"
$ErrorActionPreference = 'Stop'
try {
  $acl = Get-Acl -LiteralPath $env:LIGHTREMOTE_ACL_PATH
  $sid = [System.Security.Principal.SecurityIdentifier]
  $me = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
  $trusted = @($me, 'S-1-5-18', 'S-1-5-32-544')
  if ($env:LIGHTREMOTE_ACL_ACCESS -eq 'ancestor') {
    $trusted += 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464' # TrustedInstaller
  }
  if ($trusted -notcontains $acl.GetOwner($sid).Value) { exit 1 }
  $raw = [System.Security.AccessControl.RawSecurityDescriptor]::new($acl.GetSecurityDescriptorBinaryForm(), 0)
  if ($null -eq $raw.DiscretionaryAcl) { exit 1 }
  foreach ($rule in $acl.GetAccessRules($true, $true, $sid)) {
    if ($rule.AccessControlType -ne 'Allow') { continue }
    if (($rule.PropagationFlags -band [System.Security.AccessControl.PropagationFlags]::InheritOnly) -ne 0) { continue }
    if ($trusted -contains $rule.IdentityReference.Value) { continue }
    if ($env:LIGHTREMOTE_ACL_ACCESS -eq 'private') { exit 1 }
    # Ancestors may permit creating unrelated children (e.g. ProgramData),
    # but must not permit deleting or changing existing trusted children.
    $mask = if ($env:LIGHTREMOTE_ACL_ACCESS -eq 'ancestor') { 0xD0040 } else { 0xD0156 }
    if (([long]$rule.FileSystemRights -band $mask) -ne 0) { exit 1 }
  }
  exit 0
} catch { exit 1 }
"#;
    let system_root = std::env::var_os("SystemRoot").context("SystemRoot is unavailable")?;
    let status = std::process::Command::new(
        Path::new(&system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
    )
    .args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ])
    .env("LIGHTREMOTE_ACL_PATH", path)
    .env(
        "LIGHTREMOTE_ACL_ACCESS",
        match access {
            Access::Ancestor => "ancestor",
            Access::Parent => "parent",
            Access::Private => "private",
        },
    )
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .status()
    .context("cannot inspect Windows ACL")?;
    ensure!(
        status.success(),
        "unsafe Windows ACL; restrict access to the service account, SYSTEM and Administrators"
    );
    Ok(())
}
