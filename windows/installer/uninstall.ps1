# ZIP installations only. MSI installations should be removed through Windows Apps.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'setup-common.ps1')
if (-not (Test-Admin)) { throw 'Run this from an Administrator PowerShell.' }
$installed = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -ErrorAction Stop
if ($installed.InstallKind -ne 'zip') { throw 'Use Windows Settings > Apps to remove the MSI installation.' }
Assert-UserSid $installed.InstalledUserSid
Assert-ProtectedInstallPath $PSScriptRoot
Stop-NTPods
Unregister-NTPodsTasks
Uninstall-NTPodsDrivers $PSScriptRoot
Remove-RunValues $installed.InstalledUserSid @('NTPods', 'NTPods Daemon')
foreach ($name in 'InstalledUserSid', 'InstallKind') {
    Remove-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -Name $name -ErrorAction SilentlyContinue
}
$target = [IO.Path]::GetFullPath($PSScriptRoot).TrimEnd('\')
$expected = [IO.Path]::GetFullPath((Join-Path $env:ProgramFiles 'NTPods')).TrimEnd('\')
if ($target -ine $expected) { throw 'Refusing to remove an unexpected install directory.' }
Remove-Item -LiteralPath $target -Recurse -Force
Write-Host 'NTPods programs, tasks, drivers and development certificate removed. User data and boot settings were preserved.'
