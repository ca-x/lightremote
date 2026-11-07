#Requires -Version 5.1
# No service registration, elevation, daemon execution, or system ACL changes.
$ErrorActionPreference = 'Stop'
$installer = Join-Path $PSScriptRoot '..\windows-service.ps1'
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -ne 0) { throw 'Installer PowerShell syntax is invalid.' }
# Loading only definitions avoids the installer's #Requires and executable statements.
foreach ($definition in $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
    . ([scriptblock]::Create($definition.Extent.Text))
}

$script:checks = 0
function Assert-Rejected {
    param([scriptblock]$Action, [string]$Label)
    $rejected = $false
    try { $null = & $Action } catch { $rejected = $true }
    if (-not $rejected) { throw "Expected rejection: $Label" }
    $script:checks++
}
function Assert-Accepted {
    param([scriptblock]$Action, [string]$Label)
    try { $null = & $Action } catch { throw "Expected acceptance: $Label ($($_.Exception.Message))" }
    $script:checks++
}
function New-Descriptor {
    param([string]$Sddl)
    $acl = [System.Security.AccessControl.DirectorySecurity]::new()
    $acl.SetSecurityDescriptorSddlForm($Sddl)
    return $acl
}

$safe = 'O:SYG:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)'
foreach ($mode in @('Binary', 'Parent', 'Private', 'Ancestor')) {
    Assert-Accepted { Assert-ServiceDescriptor (New-Descriptor $safe) $mode } "SYSTEM/Administrators $mode"
}
$readable = $safe + '(A;;0x1200a9;;;BU)'
Assert-Accepted { Assert-ServiceDescriptor (New-Descriptor $readable) Binary } 'public read/execute on binary'
Assert-Rejected { Assert-ServiceDescriptor (New-Descriptor $readable) Private } 'private file may not be public-readable'
foreach ($mask in @('0x2', '0x4', '0x10', '0x100', '0x10000', '0x40000', '0x80000')) {
    $descriptor = $safe + "(A;;$mask;;;BU)"
    Assert-Rejected { Assert-ServiceDescriptor (New-Descriptor $descriptor) Binary } "binary mutation right $mask"
}
foreach ($right in @('GA', 'GW')) {
    $descriptor = $safe + "(A;;$right;;;BU)"
    foreach ($mode in @('Binary', 'Parent', 'Ancestor')) {
        Assert-Rejected { Assert-ServiceDescriptor (New-Descriptor $descriptor) $mode } "generic mutation right $right on $mode"
    }
}
Assert-Rejected { Assert-ServiceDescriptor (New-Descriptor ($safe + '(A;;0x40;;;BU)')) Ancestor } 'DELETE_CHILD ancestor'
Assert-Rejected { Assert-ServiceDescriptor (New-Descriptor ($safe + '(A;;0x2;;;BU)')) Parent } 'writable immediate parent'
Assert-Accepted { Assert-ServiceDescriptor (New-Descriptor ($safe + '(A;;0x4;;;BU)')) Ancestor } 'shared ancestor can create unrelated directories'
Assert-Rejected { Assert-ServiceDescriptor (New-Descriptor ($safe + '(A;OICIIO;FA;;;BU)')) Private } 'untrusted inheritable state grants'
Assert-Rejected { Assert-ServiceDescriptor (New-Descriptor 'O:SYG:SY') Binary } 'null DACL'
$personal = 'S-1-5-21-111111111-222222222-333333333-1001'
$descriptor = "O:${personal}G:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)"
Assert-Rejected { Assert-ServiceDescriptor (New-Descriptor $descriptor) Private } 'personal SID owner (not Administrators group)'
$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
if ($identity.User.Value -notin @('S-1-5-18', 'S-1-5-32-544')) {
    $descriptor = 'O:' + $identity.User.Value + 'G:SYD:P(A;;FA;;;SY)(A;;FA;;;BA)'
    Assert-Rejected { Assert-ServiceDescriptor (New-Descriptor $descriptor) Private } 'installer identity is not automatically trusted'
}

foreach ($path in @('relative.exe', '\\server\share\daemon.exe', 'C:daemon.exe', '\\?\C:\daemon.exe',
        'C:\safe\..\daemon.exe', 'C:\safe\daemon.exe:stream', 'C:\safe.\daemon.exe', 'C:\safe\daemon.exe ')) {
    Assert-Rejected { Get-ServiceLocalPath $path } 'ambiguous or nonlocal path'
}

# Non-admin runners still exercise real filesystem rejection without changing ownership.
$canary = Join-Path ([System.IO.Path]::GetTempPath()) ('lightremote-untrusted-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $canary
try {
    $file = Join-Path $canary 'daemon.exe'
    [System.IO.File]::WriteAllText($file, 'This canary must never be executed.')
    $acl = Get-Acl -LiteralPath $file
    $users = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-545')
    $acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new($users, 'Write', 'Allow'))
    Set-Acl -LiteralPath $file -AclObject $acl
    Assert-Rejected { Assert-ServicePath $file Binary } 'real untrusted/writable file canary'
} finally { Remove-Item -LiteralPath $canary -Recurse -Force }

function Set-FixtureAcl {
    param([string]$Path, [switch]$Directory, [string]$ExtraRights, [string]$Owner = 'S-1-5-32-544')
    $acl = if ($Directory) { [System.Security.AccessControl.DirectorySecurity]::new() } else { [System.Security.AccessControl.FileSecurity]::new() }
    $acl.SetAccessRuleProtection($true, $false)
    $acl.SetOwner([System.Security.Principal.SecurityIdentifier]::new($Owner))
    $inherit = if ($Directory) { 'ContainerInherit, ObjectInherit' } else { 'None' }
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544')) {
        $acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new(
            [System.Security.Principal.SecurityIdentifier]::new($sid), 'FullControl', $inherit, 'None', 'Allow'))
    }
    if ($ExtraRights) {
        $acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new(
            [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-545'), $ExtraRights, 'Allow'))
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
}

$principal = [System.Security.Principal.WindowsPrincipal]::new($identity)
if ($principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
    # Only this new disposable subtree receives ACL changes. No ProgramData/root ACL changes.
    $fixture = Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) ('LightRemoteDaemonTest-' + [guid]::NewGuid().ToString('N'))
    $junction = Join-Path $fixture 'junction'
    $null = New-Item -ItemType Directory -Path $fixture
    try {
        Set-FixtureAcl $fixture -Directory
        $bin = Join-Path $fixture 'bin'
        $private = Join-Path $fixture 'private'
        $null = New-Item -ItemType Directory -Path $bin, $private
        Set-FixtureAcl $bin -Directory
        Set-FixtureAcl $private -Directory
        $binary = Join-Path $bin 'daemon.exe'
        [System.IO.File]::WriteAllText($binary, 'This fixture must never be executed.')
        Set-FixtureAcl $binary
        $config = Join-Path $private 'daemon.json'
        $state = Join-Path $private 'state'
        [System.IO.File]::WriteAllText($config, (@{ state_dir = $state } | ConvertTo-Json))
        Set-FixtureAcl $config
        Assert-Accepted { Assert-ServicePath $binary Binary } 'protected binary and ancestry'
        Assert-Accepted { Assert-ServiceConfiguration $config } 'protected config and absent private state'
        if (Test-Path -LiteralPath $state) { throw 'Preflight must not create state.' }
        $null = New-Item -ItemType Directory -Path $state
        Set-FixtureAcl $state -Directory
        Assert-Accepted { Assert-ServiceConfiguration $config } 'existing protected state'

        Set-FixtureAcl $binary -ExtraRights Write
        Assert-Rejected { Assert-ServicePath $binary Binary } 'real writable binary'
        Set-FixtureAcl $binary
        Set-FixtureAcl $bin -Directory -ExtraRights DeleteSubdirectoriesAndFiles
        Assert-Rejected { Assert-ServicePath $binary Binary } 'real replaceable parent'
        Set-FixtureAcl $bin -Directory -ExtraRights CreateFiles
        Assert-Rejected { Assert-ServicePath $binary Binary } 'real writable immediate parent'
        Set-FixtureAcl $bin -Directory
        Set-FixtureAcl $config -Owner $identity.User.Value
        Assert-Rejected { Assert-ServiceConfiguration $config } 'real installer-personal SID owner'
        Set-FixtureAcl $config
        Set-FixtureAcl $state -Directory -ExtraRights ReadAndExecute
        Assert-Rejected { Assert-ServiceConfiguration $config } 'real public-readable state'
        Set-FixtureAcl $state -Directory

        $null = New-Item -ItemType Junction -Path $junction -Target $bin
        Assert-Rejected { Assert-ServicePath (Join-Path $junction 'daemon.exe') Binary } 'junction ancestor'
        Assert-Rejected { Assert-ServicePath $junction Private -Directory } 'junction leaf'
        Assert-Accepted { Assert-ServicePath $binary Binary } 'safe path remains valid after rejected canaries'
    } finally {
        if (Test-Path -LiteralPath $junction) { [System.IO.Directory]::Delete($junction) }
        Remove-Item -LiteralPath $fixture -Recurse -Force
    }
    Write-Output 'PASS protected temporary paths: safe binary/config/state, writable file/parent, personal owner, junction rejection; no SCM calls.'
} else {
    Write-Output 'SKIP protected subtree (no administrator token); pure descriptors and real untrusted path canary completed without elevation.'
}
Write-Output "PASS $script:checks Windows installer assertions; only parsed function definitions were loaded; no daemon execution or service registration."
