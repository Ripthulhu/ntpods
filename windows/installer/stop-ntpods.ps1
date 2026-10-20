# Also embedded in MSI preflight, so upgrades stop safely before an older uninstaller runs.
function Get-NTPodsSetupProcesses {
    $root = Join-Path $env:ProgramFiles 'NTPods'
    $paths = @((Join-Path $root 'ntpodsd.exe'), (Join-Path $root 'winui\ntpods-winui.exe'))
    foreach ($process in Get-Process -Name 'ntpodsd', 'ntpods-winui' -ErrorAction SilentlyContinue) {
        if ($process.HasExited) { continue }
        if (-not $process.Path) { throw 'Setup could not identify a running NTPods process. Quit NTPods and retry.' }
        if ($process.Path -in $paths) { $process }
    }
}

function Send-NTPodsSetupShutdown([int]$processId, [int]$timeoutMilliseconds) {
    if (-not ('NTPodsSetupPipe' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;
using Microsoft.Win32.SafeHandles;
public static class NTPodsSetupPipe {
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetNamedPipeServerProcessId(SafePipeHandle pipe, out uint processId);
    public static void Shutdown(int expectedProcessId, int timeoutMilliseconds) {
        // SYSTEM is allowed by the daemon ACL. CurrentUserOnly would reject that connection.
        // Identification prevents a pipe server from impersonating the elevated installer.
        using (var pipe = new NamedPipeClientStream(".", "NTPods-cmds", PipeDirection.Out,
            PipeOptions.Asynchronous, TokenImpersonationLevel.Identification)) {
            pipe.Connect(timeoutMilliseconds);
            uint serverProcessId;
            if (!GetNamedPipeServerProcessId(pipe.SafePipeHandle, out serverProcessId))
                throw new Win32Exception();
            if (serverProcessId != expectedProcessId)
                throw new IOException("The shutdown pipe belongs to another process. Quit NTPods and retry.");
            byte[] command = Encoding.UTF8.GetBytes("{\"cmd\":\"shutdown\"}\n");
            if (!pipe.WriteAsync(command, 0, command.Length).Wait(timeoutMilliseconds))
                throw new TimeoutException("NTPods did not accept the shutdown request.");
        }
    }
}
'@
    }
    [NTPodsSetupPipe]::Shutdown($processId, $timeoutMilliseconds)
}

function Get-NTPodsSetupHelper($process) {
    $info = Get-CimInstance Win32_Process -Filter "ProcessId=$($process.Id)" -OperationTimeoutSec 3 -ErrorAction Stop
    if (-not $info.CommandLine -and -not $process.HasExited) {
        throw 'Setup could not identify the running NTPods helper. Quit NTPods and retry.'
    }
    $path = Join-Path (Join-Path $env:ProgramFiles 'NTPods') 'ntpodsd.exe'
    if ($process.Path -ne $path) { return $null }
    $executable = [regex]::Escape($path)
    if ($info.CommandLine -cmatch ('^(?i:"' + $executable + '"|' + $executable + ')\s+--handsfree-guard\s*$')) { return 'handsfree' }
    if ($info.CommandLine -cmatch ('^(?i:"' + $executable + '"|' + $executable + ')\s+--hfp-profile-guard\s+(?<target>[0-9]{1,15})\s*$') -and
        [uint64]$Matches.target -le 281474976710655) { return 'handsfree' }
    if ($info.CommandLine -cmatch ('^(?i:"' + $executable + '"|' + $executable + ')\s+--run-helper\s+(?:rename-mic|fix-driver)\s*$')) { return 'task' }
    return $null
}

function Assert-NTPodsHandsfreeRestored {
    $installKey = 'HKLM:\SOFTWARE\NTPods'
    $driverKey = 'HKLM:\SYSTEM\CurrentControlSet\Services\NTPodsAAP\Parameters'
    $installed = if (Test-Path -LiteralPath $installKey -ErrorAction Stop) { Get-ItemProperty -LiteralPath $installKey -ErrorAction Stop }
    $userSid = $installed.InstalledUserSid
    if (-not $userSid -and (Test-Path -LiteralPath $driverKey -ErrorAction Stop)) {
        $userSid = (Get-ItemProperty -LiteralPath $driverKey -ErrorAction Stop).AllowedUserSid
    }
    $unresolved = 'Setup could not locate the NTPods user profile to check hands-free recovery. Start NTPods, then retry setup.'
    # A fresh MSI has already copied its files before recording the installing user.
    if (-not $userSid) { return }
    if ($userSid -notmatch '^S-1-5-21-(\d+-){3}\d+$' -or
        (New-Object Security.Principal.SecurityIdentifier($userSid)).Value -cne $userSid) { throw $unresolved }
    # Setup can run as SYSTEM; its own LOCALAPPDATA does not belong to the installed user.
    $shellKey = "Registry::HKEY_USERS\$userSid\Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders"
    $localAppData = if (Test-Path -LiteralPath $shellKey -ErrorAction Stop) {
        (Get-ItemProperty -LiteralPath $shellKey -ErrorAction Stop).'Local AppData'
    }
    if (-not $localAppData) {
        $profileKey = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\$userSid"
        if (-not (Test-Path -LiteralPath $profileKey -ErrorAction Stop)) { throw $unresolved }
        $profile = (Get-ItemProperty -LiteralPath $profileKey -ErrorAction Stop).ProfileImagePath
        if (-not $profile) { throw $unresolved }
        $localAppData = [Environment]::ExpandEnvironmentVariables($profile).TrimEnd('\') + '\AppData\Local'
    }
    if ($localAppData -notmatch '^[A-Za-z]:\\' -or $localAppData -match '[\x00-\x1f<>"|?*%]' -or $localAppData.Substring(2).Contains(':') -or
        [IO.Path]::GetFullPath($localAppData).TrimEnd('\') -ine $localAppData.TrimEnd('\')) { throw $unresolved }
    foreach ($name in 'handsfree-restore.json', 'hfp-profile-restore.json') {
        $journal = Join-Path $localAppData ('NTPods\' + $name)
        if (Test-Path -LiteralPath $journal -ErrorAction Stop) {
            throw 'Start NTPods to restore its hands-free devices, then retry setup.'
        }
    }
}

function Stop-NTPods {
    param([ValidateRange(1, 300000)][int]$TimeoutMilliseconds = 75000)
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $running = @(Get-NTPodsSetupProcesses)
    try {
        # Retain process handles so exit checks cannot attach to a reused process ID.
        foreach ($process in $running) { $null = $process.Handle }
        foreach ($client in $running | Where-Object ProcessName -eq 'ntpods-winui') {
            # Closing its window only hides the tray client, which would respawn the daemon.
            if (-not $client.HasExited) {
                try { $client | Stop-Process -Force -ErrorAction Stop }
                catch { if (-not $client.HasExited) { throw } }
            }
            if (-not $client.WaitForExit([Math]::Max(1, [Math]::Min(5000, $TimeoutMilliseconds - $clock.ElapsedMilliseconds)))) {
                throw 'NTPods could not close its interface. Setup stopped without replacing files.'
            }
        }
        $daemons = @($running | Where-Object ProcessName -eq 'ntpodsd' | ForEach-Object {
            [pscustomobject]@{ Process = $_; Helper = Get-NTPodsSetupHelper $_ }
        })
        # The guard restores endpoints when the main daemon releases its stdin lease.
        foreach ($entry in $daemons | Sort-Object { $null -ne $_.Helper }) {
            $daemon = $entry.Process
            if ($entry.Helper -eq 'task') {
                if (-not $daemon.WaitForExit([Math]::Max(1, $TimeoutMilliseconds - $clock.ElapsedMilliseconds))) {
                    throw 'An NTPods helper is still running. Setup stopped without replacing files.'
                }
                continue
            }
            if (-not $entry.Helper -and -not $daemon.HasExited) {
                $remaining = $TimeoutMilliseconds - $clock.ElapsedMilliseconds
                if ($remaining -le 0) { throw 'NTPods shutdown timed out. Setup stopped; microphone recovery may still be running.' }
                try { Send-NTPodsSetupShutdown $daemon.Id ([Math]::Min(5000, $remaining)) }
                catch { if (-not $daemon.HasExited) { throw } }
            }
            $remaining = [Math]::Max(1, $TimeoutMilliseconds - $clock.ElapsedMilliseconds)
            if (-not $daemon.WaitForExit($remaining)) {
                if ($entry.Helper -eq 'handsfree') {
                    throw 'NTPods hands-free recovery timed out. Setup stopped without replacing files.'
                }
                throw 'NTPods shutdown timed out. Setup stopped; microphone recovery may still be running.'
            }
            if ($daemon.ExitCode -ne 0) {
                if ($entry.Helper -eq 'handsfree') {
                    throw 'NTPods could not restore the Windows hands-free devices. Reconnect the AirPods, then retry setup.'
                }
                throw 'NTPods could not finish microphone recovery. Reconnect the AirPods, then retry setup.'
            }
        }
        if (@(Get-NTPodsSetupProcesses).Count) {
            throw 'NTPods restarted during shutdown. Quit it, then retry setup.'
        }
        Assert-NTPodsHandsfreeRestored
    } finally {
        foreach ($process in $running) { $process.Dispose() }
    }
}
