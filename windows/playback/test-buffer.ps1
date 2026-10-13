# Run against a test build while audio is already playing through NTPods.
[CmdletBinding(DefaultParameterSetName = 'Trial')]
param(
    [Parameter(Mandatory, ParameterSetName = 'Trial')]
    [ValidateSet(60, 80, 100, 120, 140)] [int]$TargetMs,
    [Parameter(Mandatory, ParameterSetName = 'Trial')]
    [ValidateSet(80, 100, 120, 140, 150, 300, 500)] [int]$RestoreMs,
    [Parameter(Mandatory, ParameterSetName = 'SelfTest')] [switch]$SelfTest
)
$ErrorActionPreference = 'Stop'

function Trial-Command([int]$target, [int]$restore) {
    if ($target -ge $restore) { throw 'The restore value must exceed the trial target.' }
    @{ cmd = 'set_playback_buffer'; target_ms = $target; restore_ms = $restore } | ConvertTo-Json -Compress
}
function Read-State($reader, [int]$timeout = 40000) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($timeout)
    do {
        $remaining = [int]($deadline - [DateTime]::UtcNow).TotalMilliseconds
        if ($remaining -le 0) { throw 'Timed out waiting for NTPods.' }
        $read = $reader.ReadLineAsync()
        if (-not $read.Wait($remaining)) { throw 'Timed out waiting for NTPods.' }
        if ($null -eq $read.Result) { throw 'NTPods closed the connection.' }
        $message = $read.Result | ConvertFrom-Json
        if ($message.event -eq 'state') { return $message }
    } while ($true)
}

if ($SelfTest) {
    $command = Trial-Command 60 500 | ConvertFrom-Json
    if ($command.cmd -ne 'set_playback_buffer' -or $command.target_ms -ne 60 -or $command.restore_ms -ne 500) { throw 'Command encoding failed.' }
    $rejected = $false
    try { $null = Trial-Command 140 100 } catch { $rejected = $true }
    if (-not $rejected) { throw 'Invalid restore value accepted.' }
    $reader = [IO.StringReader]::new("{`"event`":`"overlay`"}`n{`"event`":`"state`",`"playback_delay_tenths_ms`":1500}`n")
    try {
        if ((Read-State $reader 1000).playback_delay_tenths_ms -ne 1500) { throw 'State decoding failed.' }
        $closed = $false
        try { $null = Read-State $reader 1000 } catch { $closed = $true }
        if (-not $closed) { throw 'Closed pipe accepted.' }
    } finally { $reader.Dispose() }
    Write-Output 'Buffer trial client checks passed.'
    return
}

$command = Trial-Command $TargetMs $RestoreMs
$events = [IO.Pipes.NamedPipeClientStream]::new('.', 'NTPods-events', [IO.Pipes.PipeDirection]::In, [IO.Pipes.PipeOptions]::Asynchronous)
$commands = [IO.Pipes.NamedPipeClientStream]::new('.', 'NTPods-cmds', [IO.Pipes.PipeDirection]::Out, [IO.Pipes.PipeOptions]::Asynchronous)
$reader = $null
$writer = $null
$sent = $false
try {
    $events.Connect(3000)
    $commands.Connect(3000)
    $reader = [IO.StreamReader]::new($events)
    $writer = [IO.StreamWriter]::new($commands, [Text.UTF8Encoding]::new($false))
    $writer.AutoFlush = $true
    $state = Read-State $reader 3000
    if (-not $state.playback_native_driver -or -not $state.playback_delay_reporting -or $null -eq $state.playback_delay_tenths_ms) {
        throw 'Start audio through the updated NTPods driver and wait for an earbud delay report.'
    }
    Write-Output ('Earbud-reported delay: {0:0.0} ms' -f ($state.playback_delay_tenths_ms / 10.0))
    Write-Output "Requesting $TargetMs ms for 30 seconds, then sending $RestoreMs ms to restore. This does not verify the original setting."
    $writer.WriteLine($command)
    $sent = $true
    $started = $false
    $deadline = [DateTime]::UtcNow.AddSeconds(40)
    $lastDelay = $state.playback_delay_tenths_ms
    do {
        $state = Read-State $reader ([int]($deadline - [DateTime]::UtcNow).TotalMilliseconds)
        if ($state.playback_buffer_error) { throw $state.playback_buffer_error }
        if ($state.playback_buffer_target_ms -eq $TargetMs) { $started = $true }
        if ($null -ne $state.playback_delay_tenths_ms -and $lastDelay -ne $state.playback_delay_tenths_ms) {
            $lastDelay = $state.playback_delay_tenths_ms
            Write-Output ('Earbud-reported delay: {0:0.0} ms' -f ($lastDelay / 10.0))
        }
    } while (-not $started -or $null -ne $state.playback_buffer_target_ms)
    Write-Output 'Trial ended. Check daemon.log for the restore result; device application still needs verification.'
} finally {
    if ($sent -and $writer) {
        try { $writer.WriteLine('{"cmd":"set_playback_buffer","target_ms":null}') } catch { Write-Warning 'Could not request restoration. Check the NTPods log.' }
    }
    if ($writer) { $writer.Dispose() }
    if ($reader) { $reader.Dispose() }
    $commands.Dispose()
    $events.Dispose()
}
