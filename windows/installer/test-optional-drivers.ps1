# Exercise the real setup branches with privileged operations replaced by mocks.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'setup-common.ps1')
foreach ($case in @(
    @($null, $false),
    @([pscustomobject]@{ InstallKind = 'zip' }, $true),
    @([pscustomobject]@{ InstallKind = 'zip'; DirectBluetoothDrivers = 0 }, $false),
    @([pscustomobject]@{ InstallKind = 'zip'; DirectBluetoothDrivers = 1 }, $true),
    @([pscustomobject]@{ InstallKind = 'msi' }, $false)
)) {
    if ([bool](Test-NTPodsZipDriverOwnership $case[0]) -ne $case[1]) { throw 'ZIP driver ownership changed.' }
}

$tokens = $null; $parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'msi-setup.ps1'), [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw 'Invalid MSI setup script.' }
$actionSwitch = $ast.Find({ param($node) $node -is [Management.Automation.Language.SwitchStatementAst] -and $node.Condition.Extent.Text -eq '$Action' }, $true)
$branches = @{}
foreach ($clause in $actionSwitch.Clauses) {
    $body = $clause.Item2.Extent.Text
    $branches[$clause.Item1.Value] = [scriptblock]::Create($body.Substring(1, $body.Length - 2).Replace('$PSScriptRoot', '$fixture'))
}
$fixture = [IO.Path]::GetFullPath((Join-Path $env:TEMP ('ntpods-optional-drivers-' + [guid]::NewGuid())))
if (-not $fixture.StartsWith([IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe fixture path.' }
[IO.Directory]::CreateDirectory($fixture) | Out-Null
[IO.File]::WriteAllText((Join-Path $fixture 'bluetooth-host-identity.ps1'), 'param($Action, $TransactionId); Record-DriverAction "identity:$Action"')
try {
    & {
        function Record-DriverAction([string]$action) { $driverActions.Add($action) }
        function Assert-UserSid { }
        function Assert-ProtectedInstallPath { }
        function Stop-NTPods { }
        function Remove-SetupLeftovers { }
        function Set-StartupMode { }
        function New-Item { }
        function Set-ItemProperty {
            param($LiteralPath, $Name, $Value, $Type)
            if ($Name -eq 'DirectBluetoothDrivers') { $metadata['ownsDrivers'] = $Value }
        }
        function Remove-ItemProperty { }
        function Get-ItemProperty { [pscustomobject]@{ InstalledUserSid = 'S-1-5-21-1-2-3-1001' } }
        function Remove-RunValues { }
        function Assert-IdentityRemovalTransaction { $true }
        function Remove-IdentityRemoval { }
        function Test-TestMode { Record-DriverAction 'test-mode'; $script:testMode }
        function Install-NTPodsDrivers { Record-DriverAction 'install' }
        function Restore-NTPodsDriverCheckpoint { Record-DriverAction 'rollback' }
        function Complete-NTPodsDriverTransaction { Record-DriverAction 'commit' }
        function Complete-NTPodsDriverRemoval { Record-DriverAction 'remove' }
        function Register-NTPodsTasks { Record-DriverAction 'tasks:install' }
        function Unregister-NTPodsTasks { Record-DriverAction 'tasks:remove' }
        $logDir = $fixture
        $UserSid = 'S-1-5-21-1-2-3-1001'
        $TransactionId = 'optional-driver-test'
        $PreserveExisting = $true
        $PreserveDriverOwnership = $false
        $metadata = @{}
        foreach ($Drivers in $false, $true) {
            $script:testMode = $true
            foreach ($action in 'install', 'rollback', 'commit', 'finish-remove') {
                $driverActions = [Collections.Generic.List[string]]::new()
                & $branches[$action]
                $expected = if (-not $Drivers) { '' } else {
                    switch ($action) {
                        'install' { 'test-mode,install,tasks:install,identity:Install' }
                        'rollback' { 'identity:Rollback,rollback' }
                        'commit' { 'identity:Commit,commit' }
                        'finish-remove' { 'identity:Restore,remove,tasks:remove' }
                    }
                }
                if (($driverActions -join ',') -cne $expected) { throw "Wrong privileged actions for $action with Drivers=$Drivers`: $driverActions" }
            }
        }
        $Drivers = $true
        $script:testMode = $false
        $driverActions = [Collections.Generic.List[string]]::new()
        $failure = $null
        try { & $branches.install } catch { $failure = $_ }
        if (-not $failure -or $failure.ToString() -notmatch 'Test Mode is not active' -or
            ($driverActions -join ',') -cne 'test-mode') { throw 'Missing Test Mode reached driver installation.' }
        $Drivers = $false
        $PreserveDriverOwnership = $true
        $driverActions = [Collections.Generic.List[string]]::new()
        & $branches.install
        if ($driverActions.Count -ne 0) { throw 'App-only conversion changed drivers.' }
        $driverActions = [Collections.Generic.List[string]]::new()
        & $branches['finish-remove-drivers']
        if (($driverActions -join ',') -cne 'identity:Restore,remove,tasks:remove') { throw 'Removing the direct Bluetooth feature left its identity or tasks installed.' }
        if ($metadata['ownsDrivers'] -ne 0) { throw 'Committed driver removal kept ownership.' }
    }
    Write-Host 'PASS optional MSI driver install, rollback, commit, uninstall and ZIP ownership'
} finally {
    Remove-Item -LiteralPath $fixture -Recurse -Force
}
