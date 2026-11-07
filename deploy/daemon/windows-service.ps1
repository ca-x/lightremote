#Requires -RunAsAdministrator
# Explicit administrator invocation only. Never prompts for or performs elevation.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$BinaryPath,
    [Parameter(Mandatory = $true)][string]$ConfigPath
)
$ErrorActionPreference = 'Stop'

function Get-ServiceLocalPath {
    param([string]$Path)
    # Reject provider/device/UNC paths, ADS and Win32 aliases before normalization.
    if ($Path -notmatch '^[A-Za-z]:[\\/]' -or $Path.Substring(2).Contains(':') -or
        $Path -match '["\x00-\x1f]' -or $Path -match '(^|[\\/])\.\.?([\\/]|$)' -or
        $Path -match '[. ]([\\/]|$)') {
        throw 'Service paths must be unambiguous absolute local drive paths.'
    }
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    if ([System.IO.DriveInfo]::new($root).DriveType -ne [System.IO.DriveType]::Fixed) {
        throw 'Service paths must be on a fixed local drive.'
    }
    return $full
}

function Assert-ServiceDescriptor {
    param($Acl, [ValidateSet('Ancestor', 'Parent', 'Binary', 'Private')][string]$Access)
    $sid = [System.Security.Principal.SecurityIdentifier]
    # This installer always registers LocalSystem. Never trust the installer's SID.
    $trusted = @('S-1-5-18', 'S-1-5-32-544')
    if ($Access -eq 'Ancestor') {
        $trusted += 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464' # TrustedInstaller
    }
    if ($trusted -notcontains $Acl.GetOwner($sid).Value) {
        throw 'Unsafe service path owner; use SYSTEM or Administrators.'
    }
    $raw = [System.Security.AccessControl.RawSecurityDescriptor]::new($Acl.GetSecurityDescriptorBinaryForm(), 0)
    if ($null -eq $raw.DiscretionaryAcl) { throw 'Service paths must have a non-null DACL.' }
    foreach ($rule in $Acl.GetAccessRules($true, $true, $sid)) {
        if ($rule.AccessControlType -ne 'Allow') { continue }
        # Private directories must also keep future state/invite children private.
        if ($Access -ne 'Private' -and
            ($rule.PropagationFlags -band [System.Security.AccessControl.PropagationFlags]::InheritOnly) -ne 0) { continue }
        if ($trusted -contains $rule.IdentityReference.Value) { continue }
        if ($Access -eq 'Private') { throw 'Private service paths may only grant SYSTEM and Administrators access.' }
        # Shared ancestors (including stock ProgramData) may create unrelated
        # children and change their own metadata, but never delete or replace
        # existing trusted children or change ownership/access control.
        $mask = switch ($Access) {
            'Ancestor' { 0x500D0040 }
            'Parent' { 0x500D0156 }
            'Binary' { 0x500D0116 } # Also WRITE_DATA and APPEND_DATA on the executable.
        }
        if (([long]$rule.FileSystemRights -band $mask) -ne 0) {
            throw 'Service paths must not be writable or replaceable by other users.'
        }
    }
}

function Assert-ServicePath {
    param(
        [string]$Path,
        [ValidateSet('Parent', 'Binary', 'Private')][string]$Access,
        [switch]$Directory
    )
    $full = Get-ServiceLocalPath $Path
    $root = [System.IO.Path]::GetPathRoot($full)
    $parts = @($root) + @($full.Substring($root.Length).Split([char]'\') | Where-Object { $_.Length -gt 0 })
    $current = $root
    for ($index = 0; $index -lt $parts.Count; $index++) {
        if ($index -gt 0) { $current = [System.IO.Path]::Combine($current, $parts[$index]) }
        $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'Service paths must not contain symlinks or reparse points.'
        }
        $leaf = $index -eq $parts.Count - 1
        if ($item.PSIsContainer -ne (-not $leaf -or $Directory.IsPresent)) {
            throw 'Service path has the wrong file type.'
        }
        $mode = if ($leaf) { $Access } else { 'Ancestor' }
        Assert-ServiceDescriptor (Get-Acl -LiteralPath $current -ErrorAction Stop) $mode
    }
    # A trusted file ACL alone cannot prevent replacement via DELETE_CHILD on its parent.
    if (-not $Directory) {
        Assert-ServiceDescriptor (Get-Acl -LiteralPath ([System.IO.Path]::GetDirectoryName($full)) -ErrorAction Stop) Parent
    }
    return $full
}

function Assert-ServiceConfiguration {
    param([string]$Path)
    $full = Assert-ServicePath $Path Private
    try {
        # A bounded read; never let parser diagnostics disclose configuration values.
        $stream = [System.IO.File]::Open($full, 'Open', 'Read', 'Read')
        try {
            if ($stream.Length -gt 65536) { throw 'Configuration exceeds 64 KiB.' }
            $reader = [System.IO.StreamReader]::new($stream)
            try { $settings = $reader.ReadToEnd() | ConvertFrom-Json -ErrorAction Stop }
            finally { $reader.Dispose() }
        } finally { $stream.Dispose() }
        if ($settings.state_dir -isnot [string]) { throw 'Missing state directory.' }
        $state = Get-ServiceLocalPath $settings.state_dir
    } catch { throw 'Invalid service configuration; a local absolute state_dir is required (maximum 64 KiB).' }
    # Validate SYSTEM's contract before invoking the daemon under the installer token.
    if (Test-Path -LiteralPath $state) {
        $null = Assert-ServicePath $state Private -Directory
        foreach ($name in @('invite', '.invite.writer.lock', 'sessions.lock')) {
            $child = [System.IO.Path]::Combine($state, $name)
            if (Test-Path -LiteralPath $child) { $null = Assert-ServicePath $child Private }
        }
    } else {
        # A newly created state directory must inherit only private ACEs.
        $null = Assert-ServicePath ([System.IO.Path]::GetDirectoryName($state)) Private -Directory
    }
    return $full
}

# Both checks precede the first execution of the service binary. No ACL is changed.
$binary = Assert-ServicePath $BinaryPath Binary
$config = Assert-ServiceConfiguration $ConfigPath

# Validate before registration; --check-config has no SCM or network side effects.
& $binary --service --config $config --check-config
if ($LASTEXITCODE -ne 0) { throw 'Configuration validation failed; service was not created.' }
$commandLine = '"' + $binary + '" --service --config "' + $config + '"'
# Start-Process uses one explicit Windows command line. Preserve embedded quotes
# around paths on both Windows PowerShell 5.1 and PowerShell 7.
$createArguments = 'create LightRemoteDaemon binPath= "' + $commandLine.Replace('"', '\"') + '" start= delayed-auto obj= LocalSystem DisplayName= "LightRemote Daemon"'
$created = Start-Process -FilePath "$env:SystemRoot\System32\sc.exe" -ArgumentList $createArguments -NoNewWindow -Wait -PassThru
if ($created.ExitCode -ne 0) { throw 'sc.exe create failed.' }
& sc.exe description LightRemoteDaemon 'Headless unattended host. Desktop access requires a user-session CLI host.'
if ($LASTEXITCODE -ne 0) { throw 'sc.exe description failed.' }
& sc.exe failure LightRemoteDaemon reset= 86400 actions= restart/5000/restart/15000/restart/60000
if ($LASTEXITCODE -ne 0) { throw 'sc.exe failure failed.' }
# Apply recovery to a reported nonzero Stopped exit as well as a crash, including 75.
& sc.exe failureflag LightRemoteDaemon 1
if ($LASTEXITCODE -ne 0) { throw 'sc.exe failureflag failed.' }
Write-Output 'Service registered, not started. Review LocalSystem permissions, then run: sc.exe start LightRemoteDaemon'
