# Invoked by the installed elevated task. Restart only the requested AAP devnode.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'

function Read-RecoveryAddress([string]$Path, [DateTimeOffset]$Now) {
    # The request is untrusted user data, never a device ID or a command.
    try {
        for ($part = $Path; $part; $part = [IO.Path]::GetDirectoryName($part)) {
            if ([IO.File]::GetAttributes($part) -band [IO.FileAttributes]::ReparsePoint) { return }
        }
        $file = [IO.File]::Open($Path, 'Open', 'Read', 'Read')
        try {
            if ($file.Length -gt 33) { return }
            $bytes = New-Object byte[] 34
            $length = $file.Read($bytes, 0, $bytes.Length)
            $text = [Text.Encoding]::ASCII.GetString($bytes, 0, $length)
        } finally { $file.Dispose() }
    } catch { return }
    if ($text -cnotmatch '\A([0-9a-f]{12}) ([0-9]{1,20})\z' -or $Matches[1] -eq '000000000000') { return }
    $address = $Matches[1]
    [long]$issued = 0
    if (-not [long]::TryParse($Matches[2], [ref]$issued)) { return }
    $current = $Now.ToUnixTimeSeconds()
    if ($issued -gt $current -or $issued -lt $current - 30) { return }
    $address
}

function Get-RecoveryTarget([string]$Address) {
    if ($Address -cnotmatch '\A[0-9a-f]{12}\z' -or $Address -eq '000000000000') { return }
    $pattern = '^BTHENUM\\\{74ec2172-0bad-4d01-8f77-997b2be0722a\}[^\\]*\\[^\\]*&' + $Address + '_C[0-9A-F]{8}$'
    $devices = @(Get-PnpDevice -Class Bluetooth -PresentOnly -ErrorAction Stop | Where-Object {
        $_.InstanceId -match $pattern -and
        (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName 'DEVPKEY_Device_Service' -ErrorAction Stop).Data -eq 'NTPodsAAP'
    })
    if ($devices.Count -eq 1) {
        $problem = (Get-PnpDeviceProperty -InstanceId $devices[0].InstanceId -KeyName 'DEVPKEY_Device_ProblemCode' -ErrorAction Stop).Data
        if ($null -eq $problem) { throw 'The AAP device problem code is unavailable.' }
        [pscustomobject]@{ InstanceId = $devices[0].InstanceId; ProblemCode = [uint32]$problem; Status = $devices[0].Status }
    }
}

function Restart-RecoveryTarget([string]$InstanceId) {
    $systemRoot = (Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -Name SystemRoot -ErrorAction Stop).SystemRoot
    & (Join-Path $systemRoot 'System32\pnputil.exe') /restart-device $InstanceId | Out-Null
    if ($LASTEXITCODE -notin 0, 3010) { throw "NTPods AAP restart failed ($LASTEXITCODE)." }
    $LASTEXITCODE
}

function Get-RecoveryResult($Device) {
    if (-not $Device) { return 1 }
    if ($Device.ProblemCode -in 14, 38) { return 3010 }
    if ($Device.ProblemCode -eq 0 -and $Device.Status -eq 'OK') { return 0 }
    1
}

function Invoke-NTPodsDriverRecovery {
    $directory = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'NTPods'
    $request = Join-Path $directory 'driver-recovery.txt'
    $address = Read-RecoveryAddress $request ([DateTimeOffset]::UtcNow)
    if (-not $address) { return 1 }
    $device = Get-RecoveryTarget $address
    if (-not $device -or $device.ProblemCode -ne 38) { return (Get-RecoveryResult $device) }
    # Recheck after enumeration so a delayed task cannot restart a recovered pair.
    if ((Read-RecoveryAddress $request ([DateTimeOffset]::UtcNow)) -cne $address) { return 1 }
    $current = Get-RecoveryTarget $address
    if (-not $current -or $current.InstanceId -cne $device.InstanceId) { return 1 }
    if ($current.ProblemCode -ne 38) { return (Get-RecoveryResult $current) }
    if ((Restart-RecoveryTarget $current.InstanceId) -eq 3010) { return 3010 }
    # PnPUtil success only confirms the request; wait for this exact device to start.
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        Start-Sleep -Milliseconds 250
        $current = Get-RecoveryTarget $address
        if ($current -and $current.InstanceId -cne $device.InstanceId) { return 1 }
        if ((Get-RecoveryResult $current) -eq 0) { return 0 }
    }
    Get-RecoveryResult $current
}

if ($MyInvocation.InvocationName -ne '.') {
    try { exit (Invoke-NTPodsDriverRecovery) } catch { Write-Error $_; exit 1 }
}
