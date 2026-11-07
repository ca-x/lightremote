//! Cross-platform guardrails plus the native PowerShell ACL suite on Windows.
const INSTALLER: &str = include_str!("../../../deploy/daemon/windows-service.ps1");

#[test]
fn installer_checks_binary_and_system_configuration_before_execution_or_registration() {
    let binary_check = INSTALLER
        .find("$binary = Assert-ServicePath $BinaryPath Binary")
        .expect("binary must be checked before its first invocation");
    let config_check = INSTALLER
        .find("$config = Assert-ServiceConfiguration $ConfigPath")
        .expect("configuration must be checked for the target service account");
    let execute = INSTALLER.find("& $binary").unwrap();
    let register = INSTALLER.find("$created = Start-Process").unwrap();
    assert!(binary_check < config_check && config_check < execute && execute < register);
    assert!(INSTALLER.contains("$trusted = @('S-1-5-18', 'S-1-5-32-544')"));
    assert!(!INSTALLER.contains("WindowsIdentity"));
    assert!(!INSTALLER.contains("Resolve-Path"));
    assert!(INSTALLER.contains("FileAttributes]::ReparsePoint"));
    assert!(INSTALLER.contains("$null -eq $raw.DiscretionaryAcl"));
    assert!(INSTALLER.contains("obj= LocalSystem"));
}

#[cfg(windows)]
#[test]
fn native_powershell_installer_acl_tests_do_not_register_services() {
    use std::{path::Path, process::Command};

    let system_root = std::env::var_os("SystemRoot").expect("Windows requires SystemRoot");
    let powershell = Path::new(&system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/daemon/tests/windows-service.ps1");
    let output = Command::new(powershell)
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
        .arg(script)
        .output()
        .expect("Windows PowerShell must run the native ACL suite");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Windows installer assertions"));
}
