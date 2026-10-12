#requires -Version 5.1
#requires -RunAsAdministrator
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Bundle)

# Read-only. No certificate contents or device addresses appear in the report.
$ErrorActionPreference = 'Stop'
$Bundle = (Resolve-Path -LiteralPath $Bundle).Path
& (Join-Path $Bundle 'verify-bundle.ps1') -Bundle $Bundle
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$installed = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -ErrorAction SilentlyContinue
$boot = (Get-ItemProperty -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Control').SystemStartOptions
$version = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$secureBoot = try { Confirm-SecureBootUEFI -ErrorAction Stop } catch { 'Unknown' }
$memoryIntegrity = try {
    $guard = Get-CimInstance -Namespace root\Microsoft\Windows\DeviceGuard -ClassName Win32_DeviceGuard -ErrorAction Stop
    @($guard.SecurityServicesRunning) -contains 2
} catch { 'Unknown' }

[pscustomobject]@{
    Check = 'Windows'
    Build = "$($version.CurrentBuild).$($version.UBR)"
    SupportedBuild = [int]$version.CurrentBuild -ge 22621
    Architecture = $env:PROCESSOR_ARCHITECTURE
    TestModeActive = $boot -match '\bTESTSIGNING\b'
    SecureBoot = $secureBoot
    MemoryIntegrityRunning = $memoryIntegrity
    InstalledUserMatches = $installed.InstalledUserSid -eq $identity
    ExistingRollback = Test-Path -LiteralPath (Join-Path $env:ProgramData 'NTPods\driver-rollback')
    PendingPlaybackRecovery = Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'NTPods\playback-restore.json')
} | Format-List

Get-WindowsDriver -Online -ErrorAction Stop | Where-Object {
    $_.ProviderName -eq 'NTPods' -and
    (Split-Path -Leaf $_.OriginalFileName) -in 'NTPodsAAP.inf', 'AudioCodec.inf', 'NTPodsMicPC.inf'
} | Select-Object Driver, ProviderName, Version, Date | Format-Table -AutoSize

@(foreach ($service in 'NTPodsAAP', 'NTPodsMicACX') {
    $policy = Get-ItemProperty -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Services\$service\Parameters" -ErrorAction SilentlyContinue
    [pscustomobject]@{ Service = $service; UserMatches = $policy.AllowedUserSid -eq $identity }
}) | Format-Table -AutoSize

Get-PnpDevice -PresentOnly -ErrorAction Stop | Where-Object {
    $_.Class -in 'Bluetooth', 'MEDIA' -and (
        $_.InstanceId -like 'BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}*' -or
        $_.HardwareID -contains 'ROOT\NTPodsMicACX' -or
        $_.InstanceId -like 'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}*' -or
        ($_.Class -eq 'Bluetooth' -and $_.InstanceId -like 'USB\*'))
} | ForEach-Object {
    [pscustomobject]@{
        Device = $_.FriendlyName
        Status = $_.Status
        Service = (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName 'DEVPKEY_Device_Service' -ErrorAction Stop).Data
        Package = (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName 'DEVPKEY_Device_DriverInfPath' -ErrorAction Stop).Data
        Problem = (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName 'DEVPKEY_Device_ProblemCode' -ErrorAction Stop).Data
    }
} | Format-Table -AutoSize

$certificates = @(Get-ChildItem Cert:\LocalMachine\My | Where-Object {
    $_.Subject -eq 'CN=NTPods Test Cert' -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date).AddDays(30)
})
[pscustomobject]@{ Check = 'Signing'; ReusableLocalCertificate = $certificates.Count -gt 0 } | Format-List

@(foreach ($file in 'driver\NTPodsAAP.sys', 'driver\NTPodsAAP.inf', 'driver\NTPodsAAP.cat',
    'driver-mic-acx\AudioCodec.sys', 'driver-mic-acx\AudioCodec.inf', 'driver-mic-acx\AudioCodec.cat',
    'runtime\ntpodsd.exe', 'runtime\avcodec-61.dll', 'runtime\avutil-59.dll', 'runtime\swresample-5.dll',
    'setup-common.ps1', 'stop-ntpods.ps1', 'tools\devcon.exe') {
    $path = Join-Path $Bundle $file
    [pscustomobject]@{
        File = $file
        Present = Test-Path -LiteralPath $path -PathType Leaf
        SHA256 = if (Test-Path -LiteralPath $path -PathType Leaf) { (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash } else { '' }
    }
}) | Format-Table -AutoSize
