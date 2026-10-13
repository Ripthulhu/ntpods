<#
    MSI custom actions for installation, removal and driver rollback/commit.
    Runs as SYSTEM from the install folder; per-user steps use the supplied
    account SID and profile paths.

    Output goes to the MSI log and %ProgramData%\NTPods\setup-<GUID>.log.
#>
param(
    [Parameter(Mandatory)][ValidateSet('install', 'remove-drivers', 'uninstall', 'stop', 'rollback', 'commit', 'finish-remove', 'undo-remove')][string]$Action,
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
            try { & (Join-Path $PSScriptRoot 'bluetooth-host-identity.ps1') -Action Rollback -TransactionId $TransactionId }
            finally { Restore-NTPodsDriverCheckpoint $PSScriptRoot $TransactionId }
            if (-not $PreserveExisting) {
                Unregister-NTPodsTasks
                if ($UserSid) { Assert-UserSid $UserSid; Remove-RunValues $UserSid 'NTPods', 'NTPods Daemon' }
                foreach ($name in 'InstalledUserSid', 'InstallKind') {
                    Remove-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -Name $name -ErrorAction SilentlyContinue
                }
            }
        }
        'commit' {
            try { & (Join-Path $PSScriptRoot 'bluetooth-host-identity.ps1') -Action Commit -TransactionId $TransactionId }
            finally { Complete-NTPodsDriverTransaction $TransactionId }
        }
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
            & (Join-Path $PSScriptRoot 'bluetooth-host-identity.ps1') -Action Install -TransactionId $TransactionId
            # The unelevated app creates its own user-data directory.
        }
        'remove-drivers' {
            Stop-NTPods
            Uninstall-NTPodsDrivers $PSScriptRoot
        }
        'uninstall' {
            Stop-NTPods
            # Commit runs after MSI has deleted the installed scripts.
            Prepare-IdentityRemoval $PSScriptRoot $TransactionId
            Unregister-NTPodsTasks
            # The app's own "Start with Windows" setting writes these too.
            $installedSid = (Get-ItemProperty 'HKLM:\SOFTWARE\NTPods' -ErrorAction SilentlyContinue).InstalledUserSid
            if ($installedSid) { Assert-UserSid $installedSid; Remove-RunValues $installedSid 'NTPods', 'NTPods Daemon' }
            Remove-ItemProperty 'HKLM:\SOFTWARE\NTPods' -Name InstalledUserSid -ErrorAction SilentlyContinue
            Remove-ItemProperty 'HKLM:\SOFTWARE\NTPods' -Name InstallKind -ErrorAction SilentlyContinue
        }
        'finish-remove' {
            if (-not (Assert-IdentityRemovalTransaction $TransactionId)) { throw 'Identity removal files were not found.' }
            & (Join-Path $PSScriptRoot 'bluetooth-host-identity.ps1') -Action Restore
            Remove-IdentityRemoval $TransactionId
        }
        'undo-remove' { Remove-IdentityRemoval $TransactionId }
    }
    Write-Host "==> msi-setup $Action done"
    exit 0
} catch {
    Write-Host "ERROR: $_"
    Write-Host $_.ScriptStackTrace
    if ($Action -eq 'install') {
        try { & (Join-Path $PSScriptRoot 'bluetooth-host-identity.ps1') -Action Rollback -TransactionId $TransactionId }
        catch { Write-Host "Bluetooth identity recovery failed: $_ The original backup was retained." }
        try { Restore-NTPodsDriverCheckpoint $PSScriptRoot $TransactionId }
        catch { Write-Host "Driver recovery failed: $_ Backup retained at $(Get-DriverCheckpointPath)." }
    } elseif ($Action -eq 'commit') {
        Write-Host "Installation completed, but backup cleanup failed at $(Get-DriverCheckpointPath). Remaining backup files may be incomplete."
    } elseif ($Action -eq 'finish-remove') {
        Write-Host "Bluetooth identity restoration needs attention. The original backup and recovery scripts were retained at $(Get-IdentityRemovalPath)."
    }
    exit 1
} finally {
    Stop-Transcript | Out-Null
}
