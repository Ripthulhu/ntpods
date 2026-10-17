# Filesystem fixtures and mocked PnP calls only; never restarts a device.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'fix-driver.ps1')

$root = Join-Path ([IO.Path]::GetTempPath()) ('ntpods-recovery-test-' + [guid]::NewGuid())
[IO.Directory]::CreateDirectory($root) | Out-Null
$request = Join-Path $root 'driver-recovery.txt'
$now = [DateTimeOffset]::FromUnixTimeSeconds(1800000000)
$address = '001122334455'
try {
    foreach ($age in 0, 30) {
        [IO.File]::WriteAllText($request, "$address $($now.ToUnixTimeSeconds() - $age)")
        if ((Read-RecoveryAddress $request $now) -cne $address) { throw 'Valid recovery request rejected.' }
    }
    foreach ($invalid in @('', "$address 1799999969", "$address 1800000001", '000000000000 1800000000',
        '00112233445z 1800000000', '001122334455; 1800000000', "${address} 1800000000`n",
        "$address 99999999999999999999", "$address -1800000000", ('x' * 4096))) {
        [IO.File]::WriteAllText($request, $invalid)
        if (Read-RecoveryAddress $request $now) { throw 'Invalid recovery request accepted.' }
    }
    if (Read-RecoveryAddress (Join-Path $root 'missing') $now) { throw 'Missing request accepted.' }
    [IO.File]::WriteAllText($request, "$address 1800000000")
    $link = Join-Path $root 'linked'
    New-Item -ItemType Junction -Path $link -Target $root | Out-Null
    try {
        if (Read-RecoveryAddress (Join-Path $link 'driver-recovery.txt') $now) { throw 'Reparse path accepted.' }
    } finally { [IO.Directory]::Delete($link) }
} finally {
    [IO.File]::Delete($request)
    [IO.Directory]::Delete($root)
}

& {
    $script:devices = @()
    $script:restarts = @()
    $script:requestAddress = $address
    $script:reads = 0
    $script:queries = 0
    $script:changeRequest = $false
    $script:healAfterQuery = $false
    $script:restartExitCode = 0
    $script:healAfterRestart = $false
    $script:missingAfterRestart = $false
    function Read-RecoveryAddress { param($Path, $Now)
        $expected = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'NTPods\driver-recovery.txt'
        if ($Path -cne $expected) { throw 'Recovery read outside the scheduled user data directory.' }
        $script:reads++
        if ($script:changeRequest -and $script:reads -gt 1) { return 'aabbccddeeff' }
        $script:requestAddress
    }
    function Get-PnpDevice { param($Class, [switch]$PresentOnly, $ErrorAction)
        if ($Class -cne 'Bluetooth' -or -not $PresentOnly -or $ErrorAction -cne 'Stop') { throw 'Unbounded recovery query.' }
        $script:queries++
        if (($script:healAfterQuery -and $script:queries -gt 1) -or ($script:healAfterRestart -and $script:restarts.Count)) {
            $script:devices[0].Status = 'OK'; $script:devices[0].ProblemCode = 0
        }
        if ($script:missingAfterRestart -and $script:restarts.Count) { return }
        $script:devices | Where-Object { $_.Present }
    }
    function Get-PnpDeviceProperty { param($InstanceId, $KeyName, $ErrorAction)
        if ($KeyName -cnotin 'DEVPKEY_Device_Service', 'DEVPKEY_Device_ProblemCode' -or $ErrorAction -cne 'Stop') { throw 'Unexpected device property.' }
        $device = $script:devices | Where-Object { $_.InstanceId -ceq $InstanceId } | Select-Object -First 1
        [pscustomobject]@{ Data = $(if ($KeyName -ceq 'DEVPKEY_Device_Service') { $device.Service } else { $device.ProblemCode }) }
    }
    function Restart-RecoveryTarget { param($InstanceId) $script:restarts += $InstanceId; $script:restartExitCode }
    function Start-Sleep { param($Milliseconds) }
    function Device([string]$Address, [string]$Status = 'Error', [bool]$Present = $true, [string]$Service = 'NTPodsAAP', [uint32]$Problem = 38) {
        [pscustomobject]@{
            InstanceId = "BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}_VID&0001004C_PID&0000\8&123&0&${Address}_C00000000"
            Status = $Status; Present = $Present; Service = $Service; ProblemCode = $(if ($Status -eq 'OK') { 0 } else { $Problem })
        }
    }
    function Check-Restarts([int]$Count, [int]$Result = 1) {
        $script:restarts = @(); $script:reads = 0; $script:queries = 0
        $actual = Invoke-NTPodsDriverRecovery
        if ($actual -ne $Result) { throw "Expected result $Result, got $actual." }
        if ($script:restarts.Count -ne $Count) { throw "Expected $Count targeted restarts." }
        if ($Count -and $script:restarts[0] -cne $script:devices[0].InstanceId) { throw 'Wrong AirPods pair restarted.' }
    }
    $script:devices = @((Device $address), (Device 'aabbccddeeff' 'OK'))
    Check-Restarts 1 3010
    $script:devices = @((Device $address), (Device 'aabbccddeeff'))
    Check-Restarts 1 3010
    $script:devices[0].Status = 'OK'
    Check-Restarts 1 3010
    $script:healAfterRestart = $true
    Check-Restarts 1 0
    $script:healAfterRestart = $false
    $script:devices = @((Device $address))
    $script:restartExitCode = 3010
    Check-Restarts 1 3010
    $script:restartExitCode = 0
    $script:missingAfterRestart = $true
    Check-Restarts 1 1
    $script:missingAfterRestart = $false
    $script:devices = @((Device $address 'OK'))
    Check-Restarts 0 0
    $script:devices = @((Device $address 'Error' $true 'NTPodsAAP' 14))
    Check-Restarts 0 3010
    foreach ($target in @((Device $address 'Unknown' $false),
        (Device $address 'Error' $true 'OtherService'), (Device 'aabbccddeeff'))) {
        $script:devices = @($target)
        Check-Restarts 0
    }
    foreach ($problem in 0, 10, 22, 31, 39, 43, 52) {
        $script:devices = @((Device $address 'Error' $true 'NTPodsAAP' $problem))
        Check-Restarts 0
    }
    $script:devices = @((Device $address), (Device $address))
    Check-Restarts 0
    foreach ($id in @('USB\001122334455', 'BTHENUM\{other}\8&001122334455_C00000000',
        "BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}\8&${address}_C00000000;bad")) {
        $target = Device $address; $target.InstanceId = $id
        $script:devices = @($target)
        Check-Restarts 0
    }
    $script:devices = @((Device $address))
    $script:healAfterQuery = $true
    Check-Restarts 0 0
    $script:healAfterQuery = $false
    $script:devices = @((Device $address))
    $script:changeRequest = $true
    Check-Restarts 0
    $script:changeRequest = $false
    $script:requestAddress = $null
    Check-Restarts 0
    if ($script:queries) { throw 'Absent request reached device enumeration.' }
    foreach ($bad in '000000000000', '00112233445z', '001122334455;bad') {
        if (Get-RecoveryTarget $bad) { throw 'Malformed target accepted.' }
    }
}

. (Join-Path $PSScriptRoot '..\daemon\src\driver_recovery_task.ps1')
& {
    $script:polls = 0
    $script:activeTask = $null
    function Start-Sleep { param($Milliseconds)
        $script:polls++
        if ($script:activeTask) {
            $script:activeTask.State = $(if ($script:polls -eq 1) { 4 } else { 3 })
            $script:activeTask.LastTaskResult = 3010
        }
    }
    function Task([int]$Result, [bool]$Fresh = $true, [int]$State = 3) {
        $task = [pscustomobject]@{ State = $State; LastRunTime = [datetime]'2026-01-01'; LastTaskResult = $Result; Fresh = $Fresh; Runs = 0 }
        $task | Add-Member ScriptMethod Run {
            param($Arguments)
            $this.Runs++
            if ($this.Fresh) { $this.LastRunTime = $this.LastRunTime.AddSeconds(1) }
        }
        $task
    }
    foreach ($result in 0, 3010, 1) {
        $task = Task $result
        if ((Wait-NTPodsRecoveryTask $task 0) -ne $result -or $task.Runs -ne 1) { throw 'Fresh task result lost.' }
    }
    foreach ($result in 0, 3010) {
        if ((Wait-NTPodsRecoveryTask (Task $result $false) 0) -ne 1) { throw 'Stale task result accepted.' }
    }
    foreach ($state in 2, 4) {
        $task = Task 0 $true $state
        if ((Wait-NTPodsRecoveryTask $task 0) -ne 1 -or $task.Runs -ne 0) { throw 'An existing run was confused with this request.' }
    }
    $task = Task 0
    $task | Add-Member ScriptMethod Run { param($Arguments)
        $this.Runs++; $this.LastRunTime = $this.LastRunTime.AddSeconds(1); $this.State = 2
    } -Force
    $script:activeTask = $task; $script:polls = 0
    if ((Wait-NTPodsRecoveryTask $task 1000) -ne 3010 -or $script:polls -ne 2) {
        throw 'A queued or running task was mistaken for completed recovery.'
    }
}
Write-Host 'Driver recovery: fresh targeted requests, numeric problem codes, verified recovery, reboot results and task completion checks passed.'
