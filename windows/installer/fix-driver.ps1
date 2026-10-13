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
    if ($devices.Count -eq 1 -and $devices[0].Status -ne 'OK') { $devices[0] }
}

function Restart-RecoveryTarget([string]$InstanceId) {
    $systemRoot = (Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -Name SystemRoot -ErrorAction Stop).SystemRoot
    & (Join-Path $systemRoot 'System32\pnputil.exe') /restart-device $InstanceId
    if ($LASTEXITCODE -notin 0, 3010) { throw "NTPods AAP restart failed ($LASTEXITCODE)." }
    if ($LASTEXITCODE -eq 3010) { Write-Host 'Windows requires a restart to recover the AAP device.' }
}

function Invoke-NTPodsDriverRecovery {
    $directory = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'NTPods'
    $request = Join-Path $directory 'driver-recovery.txt'
    $address = Read-RecoveryAddress $request ([DateTimeOffset]::UtcNow)
    if (-not $address) { return }
    $device = Get-RecoveryTarget $address
    if (-not $device) { return }
    # Recheck after enumeration so a delayed task cannot restart a recovered pair.
    if ((Read-RecoveryAddress $request ([DateTimeOffset]::UtcNow)) -cne $address) { return }
    $current = Get-RecoveryTarget $address
    if (-not $current -or $current.InstanceId -cne $device.InstanceId) { return }
    Restart-RecoveryTarget $current.InstanceId
}

if ($MyInvocation.InvocationName -ne '.') { Invoke-NTPodsDriverRecovery }
