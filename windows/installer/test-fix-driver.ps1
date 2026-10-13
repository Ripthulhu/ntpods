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
        if ($script:healAfterQuery -and $script:queries -gt 1) { $script:devices[0].Status = 'OK' }
        $script:devices | Where-Object { $_.Present }
    }
    function Get-PnpDeviceProperty { param($InstanceId, $KeyName, $ErrorAction)
        if ($KeyName -cne 'DEVPKEY_Device_Service' -or $ErrorAction -cne 'Stop') { throw 'Recovery did not verify the driver service.' }
        $device = $script:devices | Where-Object { $_.InstanceId -ceq $InstanceId } | Select-Object -First 1
        [pscustomobject]@{ Data = $device.Service }
    }
    function Restart-RecoveryTarget { param($InstanceId) $script:restarts += $InstanceId }
    function Device([string]$Address, [string]$Status = 'Error', [bool]$Present = $true, [string]$Service = 'NTPodsAAP') {
        [pscustomobject]@{
            InstanceId = "BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}_VID&0001004C_PID&0000\8&123&0&${Address}_C00000000"
            Status = $Status; Present = $Present; Service = $Service
        }
    }
    function Check-Restarts([int]$Count) {
        $script:restarts = @(); $script:reads = 0; $script:queries = 0
        Invoke-NTPodsDriverRecovery
        if ($script:restarts.Count -ne $Count) { throw "Expected $Count targeted restarts." }
        if ($Count -and $script:restarts[0] -cne $script:devices[0].InstanceId) { throw 'Wrong AirPods pair restarted.' }
    }
    $script:devices = @((Device $address), (Device 'aabbccddeeff' 'OK'))
    Check-Restarts 1
    $script:devices = @((Device $address), (Device 'aabbccddeeff'))
    Check-Restarts 1
    foreach ($target in @((Device $address 'OK'), (Device $address 'Unknown' $false),
        (Device $address 'Error' $true 'OtherService'), (Device 'aabbccddeeff'))) {
        $script:devices = @($target)
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
    Check-Restarts 0
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
Write-Host 'Driver recovery: bounded fresh requests, multiple pairs, inactive/healthy targets and rechecks passed.'
