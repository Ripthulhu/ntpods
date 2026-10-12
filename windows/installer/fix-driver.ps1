# Invoked by the installed elevated task. Never reset the radio or other audio devices.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'

$devices = @(Get-PnpDevice -Class Bluetooth -ErrorAction SilentlyContinue | Where-Object {
    $_.InstanceId -like 'BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}*' -and
    (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName 'DEVPKEY_Device_Service' -ErrorAction SilentlyContinue).Data -eq 'NTPodsAAP'
})
# Multiple paired AirPods are ambiguous: the daemon must identify its own device.
if ($devices.Count -ne 1) { Write-Host 'No unique NTPods AAP device; recovery skipped.'; exit 0 }
$device = $devices[0]
if ($device.Status -eq 'OK') { Write-Host 'NTPods AAP device is healthy.'; exit 0 }
& "$env:SystemRoot\System32\pnputil.exe" /restart-device $device.InstanceId
if ($LASTEXITCODE -notin 0, 3010) { throw "NTPods AAP restart failed ($LASTEXITCODE)." }
if ($LASTEXITCODE -eq 3010) { Write-Host 'Windows requires a restart to recover the AAP device.' }
