# Embedded in the unelevated daemon; the installed task owns the privileged work.
$ErrorActionPreference = 'Stop'

function Wait-NTPodsRecoveryTask($Task, [int]$TimeoutMilliseconds = 125000) {
    if ($Task.State -in 2, 4) { return 1 }
    $previous = $Task.LastRunTime
    $Task.Run($null) | Out-Null
    $deadline = [Diagnostics.Stopwatch]::StartNew()
    do {
        # An accepted Run call and the previous run's result are not recovery.
        if ($Task.LastRunTime -ne $previous -and $Task.State -notin 2, 4) {
            return [int]$Task.LastTaskResult
        }
        Start-Sleep -Milliseconds 250
    } while ($deadline.ElapsedMilliseconds -lt $TimeoutMilliseconds)
    1
}

if ($MyInvocation.InvocationName -ne '.') {
    try {
        $scheduler = New-Object -ComObject 'Schedule.Service'
        $scheduler.Connect()
        exit (Wait-NTPodsRecoveryTask ($scheduler.GetFolder('\').GetTask('NTPods Fix Driver')))
    } catch { exit 1 }
}
