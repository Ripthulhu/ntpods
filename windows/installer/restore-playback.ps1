# Restore the previous AirPods playback provider without removing the app or microphone.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'setup-common.ps1')
if (-not (Test-Admin)) { throw 'Run this from an Administrator PowerShell.' }
Initialize-SetupDirectory | Out-Null
Stop-NTPods
Uninstall-NativeAudioDriver $PSScriptRoot
Remove-ItemProperty -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Services\NTPodsAudio\Parameters' -Name AllowedUserSid -ErrorAction SilentlyContinue
Write-Host 'NTPods playback removed. Reopen NTPods for AirPods controls and microphone access.'
