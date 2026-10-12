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

function Test-NTPodsSetupHelper($process) {
    $info = Get-CimInstance Win32_Process -Filter "ProcessId=$($process.Id)" -OperationTimeoutSec 3 -ErrorAction Stop
    if (-not $info.CommandLine -and -not $process.HasExited) {
        throw 'Setup could not identify the running NTPods helper. Quit NTPods and retry.'
    }
    return $info.CommandLine -match '^(?:"[^"]+"|\S+)\s+--run-helper\s+(?:rename-mic|fix-driver)\s*$'
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
        foreach ($daemon in $running | Where-Object ProcessName -eq 'ntpodsd') {
            if (-not $daemon.HasExited -and (Test-NTPodsSetupHelper $daemon)) {
                if (-not $daemon.WaitForExit([Math]::Max(1, $TimeoutMilliseconds - $clock.ElapsedMilliseconds))) {
                    throw 'An NTPods helper is still running. Setup stopped without replacing files.'
                }
                continue
            }
            if (-not $daemon.HasExited) {
                $remaining = $TimeoutMilliseconds - $clock.ElapsedMilliseconds
                if ($remaining -le 0) { throw 'NTPods shutdown timed out. Setup stopped; microphone recovery may still be running.' }
                try { Send-NTPodsSetupShutdown $daemon.Id ([Math]::Min(5000, $remaining)) }
                catch { if (-not $daemon.HasExited) { throw } }
            }
            $remaining = [Math]::Max(1, $TimeoutMilliseconds - $clock.ElapsedMilliseconds)
            if (-not $daemon.WaitForExit($remaining)) {
                throw 'NTPods shutdown timed out. Setup stopped; microphone recovery may still be running.'
            }
            if ($daemon.ExitCode -ne 0) {
                throw 'NTPods could not finish microphone recovery. Reconnect the AirPods, then retry setup.'
            }
        }
        if (@(Get-NTPodsSetupProcesses).Count) {
            throw 'NTPods restarted during shutdown. Quit it, then retry setup.'
        }
    } finally {
        foreach ($process in $running) { $process.Dispose() }
    }
}
