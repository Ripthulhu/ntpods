# Local pipes and mocked processes only. Never contacts the installed daemon.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'stop-ntpods.ps1')
function Assert-Fails([scriptblock]$body, [string]$message) {
    $failed = $false
    try { & $body } catch {
        if ($message -and $_.Exception.Message -notlike $message) { throw }
        $failed = $true
    }
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
    $process = [pscustomobject]@{ Id = 42; HasExited = $false; Path = (Join-Path $env:ProgramFiles 'NTPods\ntpodsd.exe') }
    if (Get-NTPodsSetupHelper $process) { throw 'Normal daemon misidentified as a task helper.' }
    foreach ($helper in 'rename-mic', 'fix-driver') {
        $script:processCommand = '"C:\Program Files\NTPods\ntpodsd.exe" --run-helper ' + $helper
        if ((Get-NTPodsSetupHelper $process) -ne 'task') { throw 'Installed helper was not recognized.' }
    }
    $script:processCommand += ' extra'
    if (Get-NTPodsSetupHelper $process) { throw 'Unexpected helper arguments were accepted.' }
    $script:processCommand = '"C:\Program Files\NTPods\ntpodsd.exe" --handsfree-guard'
    if ((Get-NTPodsSetupHelper $process) -ne 'handsfree') { throw 'Hands-free guard was not recognized.' }
    foreach ($arguments in '--handsfree-guard extra', '--handsfree-guard=1', '--HANDSFREE-GUARD') {
        $script:processCommand = '"C:\Program Files\NTPods\ntpodsd.exe" ' + $arguments
        if (Get-NTPodsSetupHelper $process) { throw 'Unexpected guard arguments were accepted.' }
    }
    $script:processCommand = '"C:\OtherUser\NTPods\ntpodsd.exe" --handsfree-guard'
    if (Get-NTPodsSetupHelper $process) { throw 'A different executable command line was accepted.' }
    $process.Path = 'C:\OtherUser\NTPods\ntpodsd.exe'
    if (Get-NTPodsSetupHelper $process) { throw 'An uninstalled guard was accepted.' }
    $script:processCommand = $null
    Assert-Fails { Get-NTPodsSetupHelper $process }
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
            $label = if ($this.Id -eq 44) { 'guard' } else { $this.ProcessName }
            $script:calls.Add('wait:' + $label) | Out-Null
            if ($this.Id -eq 44) {
                if (@($script:running | Where-Object { $_.Id -eq 42 -and -not $_.HasExited }).Count) { throw 'Guard was awaited before the main daemon exited.' }
                if ($script:scenario -eq 'guard-timeout') { return $false }
                $this.HasExited = $true
                if ($script:scenario -eq 'guard-failure') { $this.ExitCode = 1 }
            } elseif ($this.Id -eq 43) {
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
    function Get-NTPodsSetupHelper($process) {
        switch ($process.Id) { 43 { 'task' } 44 { 'handsfree' } }
    }
    function Assert-NTPodsHandsfreeRestored {
        if (@($script:running | Where-Object { -not $_.HasExited }).Count) { throw 'Journal was checked before all processes exited.' }
    }
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
        if ($processId -ne 42 -or @($script:running | Where-Object { $_.ProcessName -eq 'ntpods-winui' -and -not $_.HasExited }).Count) { throw 'Only the main daemon may receive shutdown after the interface stops.' }
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
    foreach ($script:scenario in 'guard-complete', 'guard-timeout', 'guard-failure') {
        $script:requested = $false
        $script:calls = New-Object 'System.Collections.Generic.List[string]'
        $script:running = @((New-TestProcess 44 'ntpodsd'), (New-TestProcess 41 'ntpods-winui'), (New-TestProcess 42 'ntpodsd'))
        if ($script:scenario -eq 'guard-complete') {
            Stop-NTPods -TimeoutMilliseconds 1000
            if (($script:calls -join ',') -ne 'close:ui,wait:ntpods-winui,request:shutdown,wait:ntpodsd,wait:guard') { throw 'Setup did not stop the main daemon before its guard.' }
        } elseif ($script:scenario -eq 'guard-timeout') {
            Assert-Fails { Stop-NTPods -TimeoutMilliseconds 1000 } '*hands-free recovery timed out*'
            if ($script:running[0].HasExited) { throw 'Setup killed a recovering guard.' }
        } else {
            Assert-Fails { Stop-NTPods -TimeoutMilliseconds 1000 } '*could not restore the Windows hands-free devices*'
        }
    }
    foreach ($script:scenario in 'guard-complete', 'guard-timeout', 'guard-failure') {
        $script:requested = $false
        $script:calls = New-Object 'System.Collections.Generic.List[string]'
        $script:running = @((New-TestProcess 44 'ntpodsd'))
        if ($script:scenario -eq 'guard-complete') {
            Stop-NTPods -TimeoutMilliseconds 1000
        } elseif ($script:scenario -eq 'guard-timeout') {
            Assert-Fails { Stop-NTPods -TimeoutMilliseconds 1000 } '*hands-free recovery timed out*'
        } else {
            Assert-Fails { Stop-NTPods -TimeoutMilliseconds 1000 } '*could not restore the Windows hands-free devices*'
        }
        if (($script:calls -join ',') -ne 'wait:guard') { throw 'An orphaned guard received a shutdown command.' }
    }
}
& {
    $sid = 'S-1-5-21-123-456-789-1001'
    $installKey = 'HKLM:\SOFTWARE\NTPods'
    $driverKey = 'HKLM:\SYSTEM\CurrentControlSet\Services\NTPodsAAP\Parameters'
    $shellKey = "Registry::HKEY_USERS\$sid\Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders"
    $profileKey = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\$sid"
    $journal = 'C:\TestProfile\AppData\Local\NTPods\handsfree-restore.json'
    $script:paths = @{}
    $script:unreadable = $null
    function Test-Path { param($LiteralPath, $ErrorAction)
        if ($LiteralPath -eq $script:unreadable) { throw 'Simulated access denied.' }
        $script:paths.ContainsKey($LiteralPath)
    }
    function Get-ItemProperty { param($LiteralPath, $ErrorAction)
        if (-not $script:paths.ContainsKey($LiteralPath)) { throw 'Unexpected registry read.' }
        $script:paths[$LiteralPath]
    }
    function Get-NTPodsSetupProcesses { @() }

    Stop-NTPods # A fresh install has no recorded user, even after payload files are copied.
    $script:paths[(Join-Path $env:ProgramFiles 'NTPods\ntpodsd.exe')] = $true
    $script:paths[$installKey] = [pscustomobject]@{}
    Stop-NTPods

    $script:paths[$installKey] = [pscustomobject]@{ InstalledUserSid = $sid }
    $script:paths[$shellKey] = [pscustomobject]@{ 'Local AppData' = 'C:\TestProfile\AppData\Local' }
    Stop-NTPods # An older installation without a journal can continue.
    $script:paths[$journal] = $true
    Assert-Fails { Stop-NTPods } 'Start NTPods to restore its hands-free devices, then retry setup.'
    $script:paths.Remove($journal)

    $script:paths.Remove($installKey)
    $script:paths[$driverKey] = [pscustomobject]@{ AllowedUserSid = $sid }
    $script:paths.Remove($shellKey)
    $script:paths[$profileKey] = [pscustomobject]@{ ProfileImagePath = 'C:\TestProfile' }
    Stop-NTPods
    $script:paths[$journal] = $true
    Assert-Fails { Stop-NTPods } 'Start NTPods to restore its hands-free devices, then retry setup.'
    $script:paths.Remove($journal)

    foreach ($invalidSid in 'S-1-5-18', 'S-1-5-21-123-456-789-01001', "$sid\Other") {
        $script:paths[$installKey] = [pscustomobject]@{ InstalledUserSid = $invalidSid }
        Assert-Fails { Stop-NTPods } '*could not locate the NTPods user profile*'
    }
    $script:paths[$installKey] = [pscustomobject]@{ InstalledUserSid = $sid }
    foreach ($invalidPath in 'relative', '\\server\profile', 'C:\TestProfile\..\Other', 'C:\%USERPROFILE%', 'C:\TestProfile:stream', 'C:\Invalid*Path', 'C:\Invalid?Path', 'C:\Invalid|Path') {
        $script:paths[$shellKey] = [pscustomobject]@{ 'Local AppData' = $invalidPath }
        Assert-Fails { Stop-NTPods } '*could not locate the NTPods user profile*'
    }
    $script:paths.Remove($shellKey)
    $script:paths.Remove($profileKey)
    Assert-Fails { Stop-NTPods } '*could not locate the NTPods user profile*'
    $script:paths[$shellKey] = [pscustomobject]@{ 'Local AppData' = 'C:\TestProfile\AppData\Local' }
    $script:unreadable = $journal
    Assert-Fails { Stop-NTPods } '*access denied*'
}
Write-Host 'Installer shutdown: verified pipe PID, daemon/guard ordering, recovery failures and orphaned journal checks passed.'
