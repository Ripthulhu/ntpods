<#
    Custom actions for NTPods.msi. The MSI runs this elevated, as SYSTEM, from the
    install folder, and passes who is installing (their SID and profile folders).

      -Action install         register the helper tasks,
                              and with -Drivers also test-sign + install both drivers
                              (-MicDriver acx, Windows 11 only)
      -Action remove-drivers  remove both drivers and the test certificate
      -Action uninstall       stop NTPods, remove the helper tasks and startup entries
      -Action stop            stop NTPods so its files can be replaced

    Output goes to the MSI log and to %ProgramData%\NTPods\setup-<GUID>.log.
#>
param(
    [Parameter(Mandatory)][ValidateSet('install', 'remove-drivers', 'uninstall', 'stop', 'rollback', 'commit')][string]$Action,
    [string]$UserSid,
    [string]$LocalAppData,
    [string]$AppData,
    [switch]$Drivers,
    [switch]$PreserveExisting,
    [string]$TransactionId,
    [ValidateSet('on', 'off')][string]$StartupMode = 'off',
    [ValidateSet('auto', 'acx')][string]$MicDriver = 'acx'
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'setup-common.ps1')

$logDir = Initialize-SetupDirectory
Start-Transcript -Path (Join-Path $logDir "setup-$([guid]::NewGuid()).log") | Out-Null
try {
    Write-Host "==> msi-setup $Action (user $UserSid, drivers: $Drivers, mic: $MicDriver)"
    switch ($Action) {
        'stop' {
            Stop-NTPods
        }
        'rollback' {
            Stop-NTPods
            Restore-NTPodsDriverCheckpoint $PSScriptRoot $TransactionId
            if (-not $PreserveExisting) {
                Unregister-NTPodsTasks
                if ($UserSid) { Assert-UserSid $UserSid; Remove-RunValues $UserSid 'NTPods', 'NTPods Daemon' }
                foreach ($name in 'InstalledUserSid', 'InstallKind') {
                    Remove-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -Name $name -ErrorAction SilentlyContinue
                }
            }
        }
        'commit' { Complete-NTPodsDriverTransaction $TransactionId }
        'install' {
            Assert-UserSid $UserSid
            Assert-ProtectedInstallPath $PSScriptRoot
            Stop-NTPods
            Remove-SetupLeftovers $LocalAppData
            if ($Drivers) {
                if (-not (Test-TestMode)) { throw 'Test Mode is not active; the drivers would not load.' }
                $work = Join-Path $logDir "drivers-$([guid]::NewGuid())"
                try { Install-NTPodsDrivers $PSScriptRoot $work $MicDriver $UserSid $TransactionId }
                finally {
                    if ([IO.Path]::GetFullPath($work).StartsWith($logDir + '\', [StringComparison]::OrdinalIgnoreCase)) {
                        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
                    }
                }
            }
            Register-NTPodsTasks $PSScriptRoot $UserSid
            New-Item -Path 'HKLM:\SOFTWARE\NTPods' -Force | Out-Null
            Set-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -Name InstalledUserSid -Value $UserSid
            Set-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -Name InstallKind -Value 'msi'
            Set-StartupMode $UserSid $StartupMode $PSScriptRoot
            # The unelevated app creates its own user-data directory.
        }
        'remove-drivers' {
            Stop-NTPods
            Uninstall-NTPodsDrivers $PSScriptRoot
        }
        'uninstall' {
            Stop-NTPods
            Unregister-NTPodsTasks
            # The app's own "Start with Windows" setting writes these too.
            $installedSid = (Get-ItemProperty 'HKLM:\SOFTWARE\NTPods' -ErrorAction SilentlyContinue).InstalledUserSid
            if ($installedSid) { Assert-UserSid $installedSid; Remove-RunValues $installedSid 'NTPods', 'NTPods Daemon' }
            Remove-ItemProperty 'HKLM:\SOFTWARE\NTPods' -Name InstalledUserSid -ErrorAction SilentlyContinue
            Remove-ItemProperty 'HKLM:\SOFTWARE\NTPods' -Name InstallKind -ErrorAction SilentlyContinue
        }
    }
    Write-Host "==> msi-setup $Action done"
    exit 0
} catch {
    Write-Host "ERROR: $_"
    Write-Host $_.ScriptStackTrace
    if ($Action -eq 'install') {
        try { Restore-NTPodsDriverCheckpoint $PSScriptRoot $TransactionId }
        catch { Write-Host "Driver recovery failed: $_ Backup retained at $(Get-DriverCheckpointPath)." }
    } elseif ($Action -eq 'commit') {
        Write-Host "Installation completed, but backup cleanup failed at $(Get-DriverCheckpointPath). Remaining backup files may be incomplete."
    }
    exit 1
} finally {
    Stop-Transcript | Out-Null
}
