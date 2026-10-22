# A restricted MSI session cannot install files, run scripts or change this PC.
param([Parameter(Mandatory)][string]$Msi)
$ErrorActionPreference = 'Stop'
$installer = New-Object -ComObject WindowsInstaller.Installer
$path = (Resolve-Path -LiteralPath $Msi).Path
$session = $null
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
    foreach ($choice in @(
        @{ AddLocal = 'Main,Drivers'; Zip = ''; Owned = ''; Expected = $true },
        @{ AddLocal = 'Main,Drivers,MicAcx'; Zip = ''; Owned = ''; Expected = $true },
        @{ AddLocal = ''; Zip = 'zip'; Owned = ''; Expected = $true },
        @{ AddLocal = ''; Zip = 'zip'; Owned = '#0'; Expected = $false },
        @{ AddLocal = ''; Zip = 'zip'; Owned = '#1'; Expected = $true }
    )) {
        $choiceSession = $installer.OpenPackage($path, 1)
        try {
            foreach ($pair in @(@('ACTION', 'INSTALL'), @('ADDLOCAL', $choice.AddLocal), @('EXISTINGINSTALL', $choice.Zip), @('OWNEDDRIVERS', $choice.Owned), @('OSBUILD', '22621'))) {
                $choiceSession.GetType().InvokeMember('Property', 'SetProperty', $null, $choiceSession, $pair)
            }
            foreach ($action in 'CostInitialize', 'FileCost', 'CostFinalize') {
                if ($choiceSession.DoAction($action) -ne 1) { throw 'Feature selection costing failed.' }
            }
            foreach ($feature in 'Drivers', 'MicAcx') {
                if (($choiceSession.FeatureRequestState($feature) -eq 3) -ne $choice.Expected) { throw "Wrong $feature selection for explicit or ZIP upgrade choice." }
            }
        } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($choiceSession) }
    }
    $session = $installer.OpenPackage($path, 1)
    $sequence = @{}
    foreach ($row in (Read-Table 'SELECT `Action`, `Condition`, `Sequence` FROM `InstallExecuteSequence`' 3)) {
        $sequence[$row[0]] = @{ Condition = $row[1]; Order = [int]$row[2] }
    }
    $actions = @{}
    foreach ($row in (Read-Table 'SELECT `Action`, `Type`, `Source`, `Target` FROM `CustomAction`' 4)) {
        $actions[$row[0]] = @{ Type = [int]$row[1]; Property = $row[2]; Value = $row[3] }
    }
    if ($actions.NTPodsStop.Type -band 64) { throw 'MSI must stop on graceful-shutdown failure.' }
    if ($actions.RequireDriverTestMode.Type -ne 19 -or
        $sequence.RequireDriverTestMode.Order -le $sequence.CostFinalize.Order -or
        $sequence.RequireDriverTestMode.Order -ge $sequence.NTPodsPreflight.Order) { throw 'Test Mode must be checked after feature selection and before any setup action.' }
    foreach ($row in (Read-Table 'SELECT `Condition` FROM `LaunchCondition`' 1)) {
        if ($row[0] -match 'TESTSIGNING') { throw 'A global Test Mode condition blocks app-only installation.' }
    }
    if (-not $sequence.ContainsKey('MigrateFeatureStates')) { throw 'Major upgrades must preserve the installed driver feature choice.' }
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
    if ($actions.RememberDriverOwnership.Type -ne 51 -or
        $sequence.RememberDriverOwnership.Order -le $sequence.CostFinalize.Order -or
        $sequence.RememberDriverOwnership.Order -ge $sequence.RemoveExistingProducts.Order) { throw 'Driver ownership must be saved before the old feature marker is removed.' }
    foreach ($row in (Read-Table 'SELECT `Name`, `Root` FROM `Registry`' 2)) {
        if ($row[1] -eq '1' -and $row[0] -in 'NTPods', 'NTPods Daemon') { throw 'MSI still owns an app-controlled startup entry.' }
    }
    foreach ($name in 'SetNTPodsInstall', 'SetNTPodsInstallDrv') {
        if (-not $actions[$name].Value.Contains('-StartupMode [STARTUPMODE]')) { throw 'Install action does not receive the saved startup choice.' }
    }
    foreach ($name in 'SetNTPodsInstall', 'SetNTPodsInstallDrv', 'SetRollbackExisting', 'SetRollbackFresh', 'SetNTPodsCommit', 'SetNTPodsUninstall', 'SetNTPodsIdentityRemoveRollback', 'SetNTPodsIdentityRemoveCommit', 'SetNTPodsRemoveDrivers', 'SetNTPodsRemoveDriversRollback', 'SetNTPodsRemoveDriversCommit') {
        if (-not $actions[$name].Value.Contains('-TransactionId "[DriverTransaction]"')) { throw "Action loses its driver transaction ownership: $name" }
    }
    if ($sequence.CaptureDriverTransaction.Order -ge $sequence.InstallInitialize.Order -or
        $actions.CaptureDriverTransaction.Type -ne 51) { throw 'Driver transaction identity must be captured once before execution.' }
    Set-Property 'ACTION' 'INSTALL'
    foreach ($action in 'CostInitialize', 'FileCost', 'CostFinalize') { Invoke-SafeAction $action }
    if ($session.EvaluateCondition('&Startup=3') -eq 1) { throw 'New installations unexpectedly enable startup by default.' }
    foreach ($feature in 'Drivers', 'MicAcx') {
        if ($session.EvaluateCondition("&$feature=3") -eq 1) { throw "New app-only installations select $feature by default." }
    }
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
                Set-Property 'OwnsDrivers' ''
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
                Set-Property 'DriverInstallArgs' ''
                if ($session.EvaluateCondition($sequence.SetDriverInstallArgs.Condition) -eq 1) { Invoke-SafeAction 'SetDriverInstallArgs' }
                foreach ($boot in '', 'NOEXECUTE=OPTIN TESTSIGNING') {
                    Set-Property 'BOOTOPTIONS' $boot
                    $reject = $session.EvaluateCondition($sequence.RequireDriverTestMode.Condition)
                    if (($reject -eq 1) -ne ($expectDrivers -and -not $boot)) { throw 'Test Mode validation disagrees with driver installation.' }
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
                Invoke-SafeAction 'SetNTPodsCommit'
                foreach ($name in 'NTPodsRollback', 'NTPodsCommit') {
                    if (($session.Property($name) -match '(?i)\s-Drivers(?:\s|$)') -ne $expectDrivers) { throw "App-only $name would touch drivers or identity." }
                }
                Set-Property 'REMOVE' 'ALL'
                if ($session.EvaluateCondition($sequence.RequireDriverTestMode.Condition) -ne 0) { throw 'Uninstall requires Test Mode.' }
                foreach ($current in 2, 3) {
                    Set-Property 'OwnsDrivers' $(if ($current -eq 3) { '1' } else { '' })
                    $condition = $sequence.ScheduleReboot.Condition.Replace('!Drivers', [string]$current).Replace('&Drivers', '2')
                    if (($session.EvaluateCondition($condition) -eq 1) -ne ($current -eq 3)) { throw 'Uninstall reboot must depend on owned drivers.' }
                    Set-Property 'DriverRemoveArgs' ''
                    $condition = $sequence.SetDriverRemoveArgs.Condition.Replace('!Drivers', [string]$current)
                    if ($session.EvaluateCondition($condition) -eq 1) { Invoke-SafeAction 'SetDriverRemoveArgs' }
                    Invoke-SafeAction 'SetNTPodsIdentityRemoveCommit'
                    if (($session.Property('NTPodsIdentityRemoveCommit') -match '(?i)\s-Drivers(?:\s|$)') -ne ($current -eq 3)) { throw 'App-only uninstall would change drivers or identity.' }
                }
                if ($session.EvaluateCondition($sequence.NTPodsRollback.Condition) -ne 0 -or
                    $session.EvaluateCondition($sequence.NTPodsCommit.Condition) -ne 0) { throw 'Uninstall schedules install rollback or commit.' }
                $driverCases++
            }
        }
    }
    Write-Host "MSI driver preservation passed $driverCases install, repair and upgrade cases."
    Set-Property 'OwnsDrivers' ''
    Set-Property 'WIX_UPGRADE_DETECTED' '1'
    Set-Property 'APPONLYUPGRADE' '1'
    Set-Property 'REMOVE' ''
    if ($session.EvaluateCondition($sequence.ScheduleReboot.Condition) -ne 0) { throw 'An app-only upgrade requests an identity restart.' }
    Set-Property 'REMOVE' 'ALL'
    Set-Property 'UPGRADINGPRODUCTCODE' '{11111111-1111-1111-1111-111111111111}'
    foreach ($name in 'NTPodsUninstall', 'NTPodsIdentityRemoveRollback', 'NTPodsIdentityRemoveCommit', 'ScheduleReboot') {
        if ($session.EvaluateCondition($sequence[$name].Condition) -ne 0) { throw "Major-upgrade removal must not restore identity or request a reboot: $name" }
    }
    Set-Property 'UPGRADINGPRODUCTCODE' ''
    foreach ($name in 'NTPodsIdentityRemoveRollback', 'NTPodsIdentityRemoveCommit') {
        if ($session.EvaluateCondition($sequence[$name].Condition) -ne 1) { throw "Uninstall is missing its identity recovery action: $name" }
    }
    Set-Property 'REMOVE' ''
    foreach ($name in 'NTPodsIdentityRemoveRollback', 'NTPodsIdentityRemoveCommit') {
        if ($session.EvaluateCondition($sequence[$name].Condition) -ne 0) { throw "Install must not restore the identity: $name" }
    }
    if (($actions.NTPodsIdentityRemoveRollback.Type -band 0x500) -ne 0x500 -or
        ($actions.NTPodsIdentityRemoveCommit.Type -band 0x600) -ne 0x600 -or
        $sequence.NTPodsIdentityRemoveRollback.Order -ge $sequence.NTPodsUninstall.Order -or
        $sequence.NTPodsUninstall.Order -ge $sequence.RemoveFiles.Order -or
        $sequence.NTPodsIdentityRemoveCommit.Order -ge $sequence.InstallFinalize.Order -or
        -not $actions.SetNTPodsIdentityRemoveCommit.Value.Contains('[CommonAppDataFolder]NTPods\identity-removal\msi-setup.ps1')) {
        throw 'Identity removal must stage its helper before file removal and restore only when uninstall commits.'
    }
    if (-not ($sequence.NTPodsRollback.Order -gt $sequence.InstallFiles.Order -and
        $sequence.NTPodsRollback.Order -lt $sequence.NTPodsInstall.Order -and
        $sequence.NTPodsCommit.Order -gt $sequence.NTPodsInstall.Order -and
        $sequence.NTPodsCommit.Order -lt $sequence.InstallFinalize.Order)) { throw 'Driver rollback/commit is scheduled outside the installation transaction.' }
    if (($actions.NTPodsRollback.Type -band 0x500) -ne 0x500 -or
        ($actions.NTPodsCommit.Type -band 0x600) -ne 0x600) { throw 'Driver backup cleanup is not a commit action, or rollback is not a rollback action.' }
    if (($actions.NTPodsRemoveDriversRollback.Type -band 0x500) -ne 0x500 -or
        ($actions.NTPodsRemoveDriversCommit.Type -band 0x600) -ne 0x600 -or
        $sequence.NTPodsRemoveDriversRollback.Order -ge $sequence.NTPodsRemoveDrivers.Order -or
        $sequence.NTPodsRemoveDrivers.Order -ge $sequence.RemoveFiles.Order -or
        $sequence.NTPodsRemoveDriversCommit.Order -le $sequence.NTPodsRemoveDrivers.Order -or
        $sequence.NTPodsRemoveDriversCommit.Order -ge $sequence.NTPodsCommit.Order) {
        throw 'Driver removal must register rollback before changing drivers and commit before normal checkpoint cleanup.'
    }
    foreach ($upgrading in '', '1') {
        Set-Property 'UPGRADINGPRODUCTCODE' $upgrading
        foreach ($remove in '', 'ALL') {
            Set-Property 'REMOVE' $remove
            foreach ($current in 2, 3) {
                foreach ($requested in 2, 3) {
                    Set-Property 'OwnsDrivers' ''
                    $expected = $current -eq 3 -and $requested -eq 2 -and -not $upgrading
                    foreach ($name in 'NTPodsRemoveDrivers', 'NTPodsRemoveDriversRollback', 'NTPodsRemoveDriversCommit') {
                        $condition = $sequence[$name].Condition.Replace('!Drivers', [string]$current).Replace('&Drivers', [string]$requested)
                        $wanted = $expected -and ($name -ne 'NTPodsRemoveDriversCommit' -or $remove -ne 'ALL')
                        if (($session.EvaluateCondition($condition) -eq 1) -ne $wanted) { throw "Wrong removal transaction condition: $name" }
                    }
                }
            }
        }
    }
    Write-Host 'MSI removal rollback, full-uninstall commit and driver-feature commit ordering passed.'

    # Native -> app-only major upgrade: no package/identity changes, but a later
    # uninstall must still remove the drivers even with the Drivers feature absent.
    Set-Property 'UPGRADINGPRODUCTCODE' ''
    Set-Property 'WIX_UPGRADE_DETECTED' '1'
    Set-Property 'Installed' ''
    Set-Property 'EXISTINGINSTALL' 'msi'
    Set-Property 'APPONLYUPGRADE' ''
    Set-Property 'BOOTOPTIONS' ''
    $session.GetType().InvokeMember('FeatureRequestState', 'SetProperty', $null, $session, @('Drivers', 2))
    foreach ($owner in 'feature', 'record', 'none') {
        Set-Property 'OLDMSIDRIVERS' $(if ($owner -eq 'feature') { '#1' } else { '' })
        Set-Property 'OWNEDDRIVERS' $(if ($owner -eq 'record') { '#1' } else { '' })
        Set-Property 'OwnsDrivers' ''
        Set-Property 'DriverOwnershipArgs' ''
        Set-Property 'DriverRemoveArgs' ''
        Set-Property 'REMOVE' ''
        foreach ($name in 'RememberDriverOwnership', 'SetDriverOwnershipArgs', 'SetDriverRemoveArgs') {
            $condition = $sequence[$name].Condition.Replace('!Drivers', '2')
            if ($session.EvaluateCondition($condition) -eq 1) { Invoke-SafeAction $name }
        }
        Invoke-SafeAction 'SetNTPodsInstall'
        $installCommand = $session.Property('NTPodsInstall')
        if ($installCommand -match '(?i)\s-Drivers(?:\s|$)' -or
            $installCommand.Contains('-PreserveDriverOwnership') -ne ($owner -ne 'none')) { throw 'App-only conversion lost ownership or selected driver installation.' }
        foreach ($name in 'NTPodsRemoveDrivers', 'NTPodsRemoveDriversRollback', 'NTPodsRemoveDriversCommit', 'ScheduleReboot', 'RequireDriverTestMode') {
            $condition = $sequence[$name].Condition.Replace('!Drivers', '2')
            if ($session.EvaluateCondition($condition) -ne 0) { throw "App-only upgrade changed the host: $name" }
        }
        Set-Property 'WIX_UPGRADE_DETECTED' ''
        Set-Property 'REMOVE' 'ALL'
        foreach ($name in 'NTPodsRemoveDrivers', 'NTPodsRemoveDriversRollback', 'ScheduleReboot') {
            $condition = $sequence[$name].Condition.Replace('!Drivers', '2')
            if (($session.EvaluateCondition($condition) -eq 1) -ne ($owner -ne 'none')) { throw "Converted installation lost its eventual cleanup: $name" }
        }
        Invoke-SafeAction 'SetNTPodsIdentityRemoveCommit'
        if (($session.Property('NTPodsIdentityRemoveCommit') -match '(?i)\s-Drivers(?:\s|$)') -ne ($owner -ne 'none')) { throw 'Converted installation lost identity cleanup.' }
        Set-Property 'WIX_UPGRADE_DETECTED' '1'
    }
    Write-Host 'MSI native-to-app-only upgrades retain ownership without touching drivers; subsequent uninstall cleans owned drivers.'
} finally {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database)
    if ($session) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($session) }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
}
