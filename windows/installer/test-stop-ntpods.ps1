# Local pipes and mocked processes only. Never contacts the installed daemon.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'stop-ntpods.ps1')
function Assert-Fails([scriptblock]$body) {
    $failed = $false
    try { & $body } catch { $failed = $true }
    if (-not $failed) { throw 'Expected shutdown rejection.' }
}

# Compile the actual pipe client with only its pipe name changed for isolation.
$pipeName = 'ntpods-setup-test-' + [guid]::NewGuid().ToString('N')
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'stop-ntpods.ps1'), [ref]$null, [ref]$null)
$source = $ast.Find({ param($node) $node -is [Management.Automation.Language.StringConstantExpressionAst] -and $node.Value.Contains('public static class NTPodsSetupPipe') }, $true).Value
Add-Type -TypeDefinition ($source.Replace('"NTPods-cmds"', '"' + $pipeName + '"'))
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.IO.Pipes;
using System.Threading.Tasks;
public sealed class NTPodsSetupTestServer : IDisposable {
    private readonly NamedPipeServerStream pipe;
    public readonly Task<string> Received;
    public NTPodsSetupTestServer(string name) {
        pipe = new NamedPipeServerStream(name, PipeDirection.In, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous);
        Received = Task.Run(async () => {
            await pipe.WaitForConnectionAsync();
            using (var reader = new StreamReader(pipe)) return await reader.ReadLineAsync();
        });
    }
    public void Dispose() { pipe.Dispose(); }
}
'@
$server = New-Object NTPodsSetupTestServer($pipeName)
try {
    [NTPodsSetupPipe]::Shutdown($PID, 1000)
    if (-not $server.Received.Wait(1000) -or $server.Received.Result -cne '{"cmd":"shutdown"}') { throw 'Graceful shutdown command was not delivered.' }
} finally { $server.Dispose() }
$server = New-Object NTPodsSetupTestServer($pipeName)
try {
    Assert-Fails { [NTPodsSetupPipe]::Shutdown(0, 1000) }
    if (-not $server.Received.Wait(1000) -or $null -ne $server.Received.Result) { throw 'A different pipe process received the shutdown command.' }
} finally { $server.Dispose() }
Assert-Fails { [NTPodsSetupPipe]::Shutdown($PID, 20) }

& {
    $script:processCommand = '"C:\Program Files\NTPods\ntpodsd.exe"'
    function Get-CimInstance { param($ClassName, $Filter, $OperationTimeoutSec, $ErrorAction)
        if ($ClassName -ne 'Win32_Process' -or $Filter -ne 'ProcessId=42' -or $OperationTimeoutSec -ne 3) { throw 'Unexpected helper identity query.' }
        [pscustomobject]@{ CommandLine = $script:processCommand }
    }
    $process = [pscustomobject]@{ Id = 42; HasExited = $false }
    if (Test-NTPodsSetupHelper $process) { throw 'Normal daemon misidentified as a task helper.' }
    foreach ($helper in 'rename-mic', 'fix-driver') {
        $script:processCommand = '"C:\Program Files\NTPods\ntpodsd.exe" --run-helper ' + $helper
        if (-not (Test-NTPodsSetupHelper $process)) { throw 'Installed helper was not recognized.' }
    }
    $script:processCommand += ' extra'
    if (Test-NTPodsSetupHelper $process) { throw 'Unexpected helper arguments were accepted.' }
    $script:processCommand = $null
    Assert-Fails { Test-NTPodsSetupHelper $process }
}

& {
    $root = Join-Path $env:ProgramFiles 'NTPods'
    function Get-Process { param($Name, $ErrorAction)
        [pscustomobject]@{ Id = 1; HasExited = $false; Path = (Join-Path $root 'ntpodsd.exe') }
        [pscustomobject]@{ Id = 2; HasExited = $false; Path = 'C:\OtherUser\NTPods\ntpods-winui.exe' }
        [pscustomobject]@{ Id = 3; HasExited = $true; Path = $null }
    }
    $selected = @(Get-NTPodsSetupProcesses)
    if ($selected.Count -ne 1 -or $selected[0].Id -ne 1) { throw 'Shutdown selected an unrelated copy.' }
}

& {
    function New-TestProcess([int]$id, [string]$name) {
        $process = [pscustomobject]@{ Id = $id; ProcessName = $name; HasExited = $false; Handle = [IntPtr]1; ExitCode = 0 }
        $process | Add-Member ScriptMethod Dispose { }
        $process | Add-Member ScriptMethod WaitForExit {
            param($milliseconds)
            $script:calls.Add('wait:' + $this.ProcessName) | Out-Null
            if ($this.Id -eq 43) {
                if ($script:scenario -eq 'helper-timeout') { return $false }
                $this.HasExited = $true
                $this.ExitCode = 1 # A completed helper failure does not leave a live microphone stream.
            } elseif ($this.ProcessName -eq 'ntpodsd') {
                if (-not $script:requested) { throw 'Wait ran before shutdown was sent.' }
                if ($script:scenario -eq 'timeout') { return $false }
                Start-Sleep -Milliseconds 25 # A receiver can finish recovery after command delivery.
                $this.HasExited = $true
                if ($script:scenario -eq 'failure') { $this.ExitCode = 1 }
            }
            return $this.HasExited
        }
        $process
    }
    function Test-NTPodsSetupHelper($process) { $process.Id -eq 43 }
    function Get-NTPodsSetupProcesses { $script:running | Where-Object { -not $_.HasExited } }
    function Stop-Process {
        param([Parameter(ValueFromPipeline)]$InputObject, [switch]$Force)
        process {
            if ($InputObject.ProcessName -ne 'ntpods-winui') { throw 'The microphone daemon must never be force-killed.' }
            $script:calls.Add('close:ui') | Out-Null
            if ($script:scenario -eq 'ui-failure') { throw 'Simulated UI close failure.' }
            $InputObject.HasExited = $true
        }
    }
    function Send-NTPodsSetupShutdown([int]$processId, [int]$timeoutMilliseconds) {
        if ($processId -ne 42 -or -not $script:running[0].HasExited) { throw 'The interface must stop before microphone recovery.' }
        if ($script:scenario -eq 'pipe-failure') { throw 'Simulated pipe failure.' }
        $script:calls.Add('request:shutdown') | Out-Null
        $script:requested = $true
    }
    foreach ($script:scenario in 'complete', 'timeout', 'failure', 'pipe-failure', 'ui-failure') {
        $script:requested = $false
        $script:calls = New-Object 'System.Collections.Generic.List[string]'
        $script:running = @((New-TestProcess 41 'ntpods-winui'), (New-TestProcess 42 'ntpodsd'))
        if ($script:scenario -eq 'complete') {
            Stop-NTPods -TimeoutMilliseconds 1000
            if (($script:calls -join ',') -ne 'close:ui,wait:ntpods-winui,request:shutdown,wait:ntpodsd') { throw 'Setup did not wait for recovery before continuing.' }
        } else {
            Assert-Fails { Stop-NTPods -TimeoutMilliseconds 1000 }
            if ($script:scenario -in 'timeout', 'pipe-failure', 'ui-failure' -and $script:running[1].HasExited) { throw 'Failed setup killed the daemon.' }
        }
    }
    $script:running = @()
    Stop-NTPods
    foreach ($script:scenario in 'helper-complete', 'helper-timeout') {
        $script:requested = $false
        $script:calls = New-Object 'System.Collections.Generic.List[string]'
        $script:running = @((New-TestProcess 41 'ntpods-winui'), (New-TestProcess 42 'ntpodsd'), (New-TestProcess 43 'ntpodsd'))
        if ($script:scenario -eq 'helper-complete') {
            Stop-NTPods -TimeoutMilliseconds 1000
            if (($script:calls -join ',') -ne 'close:ui,wait:ntpods-winui,request:shutdown,wait:ntpodsd,wait:ntpodsd') { throw 'Setup did not wait for the helper.' }
        } else {
            Assert-Fails { Stop-NTPods -TimeoutMilliseconds 1000 }
            if ($script:running[2].HasExited) { throw 'Setup killed a running helper.' }
        }
    }
}
Write-Host 'Installer shutdown: process scope, verified pipe PID, recovery wait, nonzero exit and timeout checks passed.'
