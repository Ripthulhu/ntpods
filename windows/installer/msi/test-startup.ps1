# A restricted MSI session cannot install files, run scripts or change this PC.
param([Parameter(Mandatory)][string]$Msi)
$ErrorActionPreference = 'Stop'
$installer = New-Object -ComObject WindowsInstaller.Installer
$path = (Resolve-Path -LiteralPath $Msi).Path
$session = $installer.OpenPackage($path, 1)
$database = $installer.OpenDatabase($path, 0)
function Read-Table([string]$query, [int]$columns) {
    $view = $database.OpenView($query)
    try {
        [void]$view.Execute()
        while ($record = $view.Fetch()) {
            try {
                $row = @()
                for ($column = 1; $column -le $columns; $column++) { $row += $record.StringData($column) }
                ,$row
            } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) }
        }
    } finally { [void]$view.Close(); [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) }
}
function Set-Property([string]$name, [string]$value) {
    $session.GetType().InvokeMember('Property', 'SetProperty', $null, $session, @($name, $value))
}
function Invoke-SafeAction([string]$action) {
    if ($session.DoAction($action) -ne 1) { throw "MSI test action failed: $action" }
}
try {
    $sequence = @{}
    foreach ($row in (Read-Table 'SELECT `Action`, `Condition`, `Sequence` FROM `InstallExecuteSequence`' 3)) {
        $sequence[$row[0]] = @{ Condition = $row[1]; Order = [int]$row[2] }
    }
    $actions = @{}
    foreach ($row in (Read-Table 'SELECT `Action`, `Type`, `Source`, `Target` FROM `CustomAction`' 4)) {
        $actions[$row[0]] = @{ Type = [int]$row[1]; Property = $row[2]; Value = $row[3] }
    }
    if ($actions.NTPodsStop.Type -band 64) { throw 'MSI must stop on graceful-shutdown failure.' }
    if ($sequence.NTPodsPreflight.Order -ge $sequence.InstallValidate.Order -or
        $sequence.NTPodsPreflight.Order -ge $sequence.RemoveExistingProducts.Order -or
        $actions.NTPodsPreflight.Type -band 64) { throw 'Graceful shutdown must succeed before an older uninstaller runs.' }
    if ($session.Property('MSIRESTARTMANAGERCONTROL') -cne 'DisableShutdown') { throw 'Restart Manager must not terminate the microphone recovery.' }
    $preflightCommands = @($actions.Values | Where-Object { $_.Property -eq 'WixQuietExecCmdLine' })
    if ($preflightCommands.Count -ne 1) { throw 'Expected one embedded preflight command.' }
    $encoded = ($preflightCommands[0].Value -split '-EncodedCommand ', 2)[1]
    $preflight = [Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($encoded))
    if ($preflight -notmatch 'function Stop-NTPods' -or $preflight.TrimEnd() -notmatch 'Stop-NTPods$') {
        throw 'MSI preflight must embed and call the current shutdown helper.'
    }
    $startupActions = @('SetStartupExpectedApp', 'SetStartupExpectedDaemon', 'SetStartupDefault', 'SetStartupFresh', 'SetStartupExisting')
    foreach ($name in $startupActions) {
        if ($actions[$name].Type -ne 51 -or $sequence[$name].Order -le $sequence.CostFinalize.Order -or
            $sequence[$name].Order -ge $sequence.RemoveExistingProducts.Order) { throw "Startup snapshot is not safe before old-product removal: $name" }
    }
    foreach ($row in (Read-Table 'SELECT `Name`, `Root` FROM `Registry`' 2)) {
        if ($row[1] -eq '1' -and $row[0] -in 'NTPods', 'NTPods Daemon') { throw 'MSI still owns an app-controlled startup entry.' }
    }
    foreach ($name in 'SetNTPodsInstall', 'SetNTPodsInstallDrv') {
        if (-not $actions[$name].Value.Contains('-StartupMode [STARTUPMODE]')) { throw 'Install action does not receive the saved startup choice.' }
    }
    foreach ($name in 'SetNTPodsInstall', 'SetNTPodsInstallDrv', 'SetRollbackExisting', 'SetRollbackFresh', 'SetNTPodsCommit') {
        if (-not $actions[$name].Value.Contains('-TransactionId "[DriverTransaction]"')) { throw "Action loses its driver transaction ownership: $name" }
    }
    if ($sequence.CaptureDriverTransaction.Order -ge $sequence.InstallInitialize.Order -or
        $actions.CaptureDriverTransaction.Type -ne 51) { throw 'Driver transaction identity must be captured once before execution.' }
    Set-Property 'ACTION' 'INSTALL'
    foreach ($action in 'CostInitialize', 'FileCost', 'CostFinalize') { Invoke-SafeAction $action }
    if ($session.EvaluateCondition('&Startup=3') -eq 1) { throw 'New installations unexpectedly enable startup by default.' }
    Set-Property 'INSTALLFOLDER' 'C:\Program Files\NTPods\'
    Invoke-SafeAction 'SetStartupExpectedApp'
    Invoke-SafeAction 'SetStartupExpectedDaemon'
    $app = '"C:\Program Files\NTPods\winui\ntpods-winui.exe" --tray'
    $daemon = '"C:\Program Files\NTPods\ntpodsd.exe"'
    $cases = 0
    foreach ($existing in '', 'Installed', 'WIX_UPGRADE_DETECTED', 'EXISTINGINSTALL') {
        foreach ($entries in 'both', 'none', 'app-only', 'daemon-only', 'stale') {
            foreach ($feature in 2, 3) {
                foreach ($property in 'Installed', 'WIX_UPGRADE_DETECTED', 'EXISTINGINSTALL') { Set-Property $property '' }
                if ($existing) { Set-Property $existing '1' }
                Set-Property 'STARTUPAPP' $(if ($entries -in 'both', 'app-only') { $app } elseif ($entries -eq 'stale') { 'old.exe' } else { '' })
                Set-Property 'STARTUPDAEMON' $(if ($entries -in 'both', 'daemon-only', 'stale') { $daemon } else { '' })
                $session.GetType().InvokeMember('FeatureRequestState', 'SetProperty', $null, $session, @('Startup', $feature))
                Set-Property 'STARTUPMODE' 'untrusted-input'
                foreach ($name in 'SetStartupDefault', 'SetStartupFresh', 'SetStartupExisting') {
                    $condition = $session.EvaluateCondition($sequence[$name].Condition)
                    if ($condition -eq 3) { throw "Invalid MSI condition: $name" }
                    if ($condition -in 1, 2) { Invoke-SafeAction $name }
                }
                $expected = if (($existing -and $entries -eq 'both') -or (-not $existing -and $feature -eq 3)) { 'on' } else { 'off' }
                if ($session.Property('STARTUPMODE') -ne $expected) { throw "Wrong startup choice: existing=$existing entries=$entries feature=$feature" }
                $cases++
            }
        }
    }
    Write-Host "MSI startup preservation passed $cases install, repair and upgrade cases."
    if ('APPONLYUPGRADE' -notin $session.Property('SecureCustomProperties').Split(';')) {
        throw 'The app-only upgrade choice does not reach the elevated install session.'
    }
    $driverCases = 0
    foreach ($existing in '', 'Installed', 'WIX_UPGRADE_DETECTED') {
        foreach ($appOnly in '', '0', '1') {
            foreach ($feature in 2, 3) {
                foreach ($property in 'Installed', 'WIX_UPGRADE_DETECTED') { Set-Property $property '' }
                if ($existing) { Set-Property $existing '1' }
                Set-Property 'APPONLYUPGRADE' $appOnly
                Set-Property 'REMOVE' ''
                $session.GetType().InvokeMember('FeatureRequestState', 'SetProperty', $null, $session, @('Drivers', $feature))
                Set-Property 'NTPodsInstall' ''
                $selected = 0
                foreach ($name in 'SetNTPodsInstall', 'SetNTPodsInstallDrv') {
                    if ($actions[$name].Type -ne 51) { throw "Unsafe MSI test action: $name" }
                    $condition = $session.EvaluateCondition($sequence[$name].Condition)
                    if ($condition -eq 3) { throw "Invalid MSI condition: $name" }
                    if ($condition -in 1, 2) { Invoke-SafeAction $name; $selected++ }
                }
                $expectDrivers = $feature -eq 3 -and -not ($existing -eq 'WIX_UPGRADE_DETECTED' -and $appOnly -eq '1')
                $hasDrivers = $session.Property('NTPodsInstall') -match '(?i)\s-Drivers(?:\s|$)'
                if ($selected -ne 1 -or $hasDrivers -ne $expectDrivers) {
                    throw "Wrong driver setup choice: existing=$existing appOnly=$appOnly feature=$feature"
                }
                $reboot = $session.EvaluateCondition($sequence.ScheduleReboot.Condition)
                if ($reboot -notin 0, 1 -or ($reboot -eq 1) -ne $expectDrivers) { throw 'Driver setup and reboot choices disagree.' }
                if ($session.EvaluateCondition($sequence.NTPodsRollback.Condition) -ne 1 -or
                    $session.EvaluateCondition($sequence.NTPodsCommit.Condition) -ne 1) { throw 'Install, repair or upgrade is missing rollback/commit.' }
                Set-Property 'EXISTINGINSTALL' ''
                Set-Property 'NTPodsRollback' ''
                $selectedRollback = 0
                foreach ($name in 'SetRollbackExisting', 'SetRollbackFresh') {
                    if ($actions[$name].Type -ne 51) { throw "Unsafe MSI test action: $name" }
                    if ($session.EvaluateCondition($sequence[$name].Condition) -eq 1) { Invoke-SafeAction $name; $selectedRollback++ }
                }
                if ($selectedRollback -ne 1 -or
                    ($session.Property('NTPodsRollback').Contains('-PreserveExisting')) -ne [bool]$existing) { throw 'Rollback would remove an existing installation.' }
                Set-Property 'REMOVE' 'ALL'
                if ($session.EvaluateCondition($sequence.ScheduleReboot.Condition) -ne 0) { throw 'Uninstall schedules a driver-install reboot.' }
                if ($session.EvaluateCondition($sequence.NTPodsRollback.Condition) -ne 0 -or
                    $session.EvaluateCondition($sequence.NTPodsCommit.Condition) -ne 0) { throw 'Uninstall schedules install rollback or commit.' }
                $driverCases++
            }
        }
    }
    Write-Host "MSI driver preservation passed $driverCases install, repair and upgrade cases."
    if (-not ($sequence.NTPodsRollback.Order -gt $sequence.InstallFiles.Order -and
        $sequence.NTPodsRollback.Order -lt $sequence.NTPodsInstall.Order -and
        $sequence.NTPodsCommit.Order -gt $sequence.NTPodsInstall.Order -and
        $sequence.NTPodsCommit.Order -lt $sequence.InstallFinalize.Order)) { throw 'Driver rollback/commit is scheduled outside the installation transaction.' }
    if (($actions.NTPodsRollback.Type -band 0x500) -ne 0x500 -or
        ($actions.NTPodsCommit.Type -band 0x600) -ne 0x600) { throw 'Driver backup cleanup is not a commit action, or rollback is not a rollback action.' }
} finally {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database)
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($session)
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
}
