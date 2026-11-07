# Windows PowerShell 5.1+, launched by the current user's scheduled task.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$child = $null
try {
    $state = $PSScriptRoot
    if ((Get-Item -LiteralPath $state).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw 'Invalid state directory.'
    }
    $config = Get-Content -LiteralPath (Join-Path $state 'host.json') -Raw | ConvertFrom-Json
    if ($config.PolicyConfirmed -isnot [bool] -or -not $config.PolicyConfirmed) { throw 'Policy not confirmed.' }
    if ([string]::IsNullOrWhiteSpace($config.OperatorToken) -or $config.OperatorToken -eq 'REPLACE_WITH_OPERATOR_TOKEN') {
        throw 'Missing credential.'
    }
    foreach ($url in @($config.RelayUrl, $config.WebUrl)) {
        $uri = [Uri]$url
        if (-not $uri.IsAbsoluteUri -or $uri.Scheme -ne 'https' -or $uri.UserInfo) { throw 'Invalid URL.' }
    }
    if (-not [IO.Path]::IsPathRooted($config.BinaryPath) -or -not (Test-Path -LiteralPath $config.BinaryPath -PathType Leaf)) {
        throw 'Missing executable.'
    }
    $hostArguments = @('host', '--stay', '--unattended', '--invite-file', (Join-Path $state 'invite'),
        '--relay', $config.RelayUrl, '--web-url', $config.WebUrl)
    if ($config.ShareDir) {
        if (-not [IO.Path]::IsPathRooted($config.ShareDir) -or -not (Test-Path -LiteralPath $config.ShareDir -PathType Container)) {
            throw 'Invalid share.'
        }
        $hostArguments += @('--share-dir', $config.ShareDir)
    }
    $flags = [ordered]@{
        AllowShell = '--allow-shell'; AllowWrite = '--allow-write'; AllowDesktop = '--allow-desktop'
        AllowInput = '--allow-input'; AllowClipboard = '--allow-clipboard'
        AllowClipboardRead = '--allow-clipboard-read'; AllowClipboardWrite = '--allow-clipboard-write'
        AutoUpdate = '--auto-update'
    }
    foreach ($name in $flags.Keys) {
        if ($config.$name -isnot [bool]) { throw 'Permissions must be JSON booleans.' }
        if ($config.$name) {
            if ($name -eq 'AllowWrite' -and -not $config.ShareDir) { throw 'Write requires a share.' }
            $hostArguments += $flags[$name]
        }
    }
    # PS 5.1 has no ProcessStartInfo.ArgumentList. Quote arguments for Windows CRT;
    # reject embedded quotes/newlines, double backslashes before the closing quote.
    $quoted = foreach ($value in $hostArguments) {
        if ($value -match '["\r\n\x00]') { throw 'Invalid argument.' }
        '"' + ($value -replace '(\\+)$', '$1$1') + '"'
    }
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $config.BinaryPath
    $start.Arguments = $quoted -join ' '
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WorkingDirectory = $state
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.EnvironmentVariables['LIGHTREMOTE_RELAY_TOKEN'] = $config.OperatorToken
    $start.EnvironmentVariables.Remove('LIGHTREMOTE_INVITE')
    $start.EnvironmentVariables.Remove('RUST_LOG')
    $start.EnvironmentVariables.Remove('RUST_BACKTRACE')
    while ($true) {
        $child = New-Object Diagnostics.Process
        $child.StartInfo = $start
        [void]$child.Start()
        $child.StandardInput.Close()
        # Drain concurrently to Stream.Null: no files, console output or transcripts.
        $stdoutDrain = $child.StandardOutput.BaseStream.CopyToAsync([IO.Stream]::Null)
        $stderrDrain = $child.StandardError.BaseStream.CopyToAsync([IO.Stream]::Null)
        $child.WaitForExit()
        $stdoutDrain.GetAwaiter().GetResult()
        $stderrDrain.GetAwaiter().GetResult()
        $code = $child.ExitCode
        $child.Dispose()
        $child = $null
        # 75 means update installed/replacement helper scheduled. Give helper time.
        if ($code -eq 75) { Start-Sleep -Seconds 15 } else { Start-Sleep -Seconds 10 }
    }
} catch {
    # Never print exceptions: they may include configuration or remote data.
    [Console]::Error.WriteLine('LightRemote host: stopped; check private configuration.')
    exit 64
} finally {
    if ($null -ne $child) {
        try {
            if (-not $child.HasExited) { $child.Kill(); $child.WaitForExit() }
        } catch {
            # Process may never have started; keep all exception text private.
        } finally { $child.Dispose() }
    }
}
