# Only an explicit -Action Install registers a task. No elevation or password.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Prepare', 'Install', 'Stop', 'Uninstall')]
    [string]$Action
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$taskName = 'LightRemoteHost'
$state = Join-Path $env:LOCALAPPDATA 'LightRemoteHost'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$user = $identity.Name

if ($Action -eq 'Stop' -or $Action -eq 'Uninstall') {
    # Disable before stopping: prevents a logon trigger/restart from racing shutdown.
    Disable-ScheduledTask -TaskName $taskName | Out-Null
    Stop-ScheduledTask -TaskName $taskName
    if ($Action -eq 'Uninstall') { Unregister-ScheduledTask -TaskName $taskName -Confirm:$false }
    # Private config/invitation are intentionally retained for explicit deletion.
    return
}

if ((Test-Path -LiteralPath $state) -and
    ((Get-Item -LiteralPath $state).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw 'Refusing a redirected state directory.'
}
[void][IO.Directory]::CreateDirectory($state)
$acl = New-Object Security.AccessControl.DirectorySecurity
$acl.SetOwner($identity.User)
$acl.SetAccessRuleProtection($true, $false)
$rule = New-Object Security.AccessControl.FileSystemAccessRule(
    $identity.User, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
$acl.AddAccessRule($rule)
Set-Acl -LiteralPath $state -AclObject $acl
# Fresh files inherit the private directory ACL; refuse reparse points.
foreach ($name in @('run-host.ps1', 'host.json', 'invite')) {
    $path = Join-Path $state $name
    if ((Test-Path -LiteralPath $path) -and
        ((Get-Item -LiteralPath $path).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'Refusing redirected state files.'
    }
}
function Protect-PrivateFile([string]$Path) {
    $fileAcl = New-Object Security.AccessControl.FileSecurity
    $fileAcl.SetOwner($identity.User)
    $fileAcl.SetAccessRuleProtection($true, $false)
    $fileAcl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($identity.User, 'FullControl', 'Allow')))
    Set-Acl -LiteralPath $Path -AclObject $fileAcl
}
if ($Action -eq 'Prepare') {
    if (Test-Path -LiteralPath (Join-Path $state 'host.json')) { throw 'Configuration already exists; edit it in place.' }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'host.example.json') -Destination (Join-Path $state 'host.json')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'run-host.ps1') -Destination (Join-Path $state 'run-host.ps1')
    # Normalize each file too; an existing explicit allow must never survive.
    foreach ($name in @('host.json', 'run-host.ps1')) {
        Protect-PrivateFile (Join-Path $state $name)
    }
    Write-Output 'Prepared private configuration. Edit host.json, choose permissions, then explicitly Install.'
    return
}
foreach ($name in @('host.json', 'run-host.ps1', 'invite')) {
    $path = Join-Path $state $name
    if (Test-Path -LiteralPath $path) { Protect-PrivateFile $path }
}
try { $config = Get-Content -LiteralPath (Join-Path $state 'host.json') -Raw | ConvertFrom-Json }
catch { throw 'Invalid private JSON configuration.' }
if ($config.PolicyConfirmed -isnot [bool] -or -not $config.PolicyConfirmed) { throw 'Choose and confirm the unattended policy first.' }
if ($config.OperatorToken -eq 'REPLACE_WITH_OPERATOR_TOKEN' -or [string]::IsNullOrWhiteSpace($config.OperatorToken)) {
    throw 'Configure the credential in the private file first.'
}
if (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue) { throw 'Task exists; Uninstall before replacing it.' }
$runner = Join-Path $state 'run-host.ps1'
if (-not (Test-Path -LiteralPath $runner -PathType Leaf)) { throw 'Run Prepare first.' }
$powershell = Join-Path $PSHOME 'powershell.exe'
$taskAction = New-ScheduledTaskAction -Execute $powershell -Argument (
    '-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -File "' + $runner + '"')
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) `
    -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
$task = New-ScheduledTask -Action $taskAction -Trigger $trigger -Principal $principal -Settings $settings
Register-ScheduledTask -TaskName $taskName -InputObject $task | Out-Null
Start-ScheduledTask -TaskName $taskName
Write-Output 'LightRemote task registered for this user. Configuration is a persistent unattended policy.'
