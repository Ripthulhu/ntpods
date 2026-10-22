# Execute the install/rollback branches with mocked registry and startup operations.
$ErrorActionPreference = 'Stop'
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'msi-setup.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'Invalid MSI setup script.' }
$actionSwitch = $ast.Find({ param($node) $node -is [Management.Automation.Language.SwitchStatementAst] -and $node.Condition.Extent.Text -eq '$Action' }, $true)
$branches = @{}
foreach ($clause in $actionSwitch.Clauses) {
    $body = $clause.Item2.Extent.Text
    $branches[$clause.Item1.Value] = [scriptblock]::Create($body.Substring(1, $body.Length - 2))
}
& {
    function Assert-UserSid { }
    function Assert-ProtectedInstallPath { }
    function Stop-NTPods { }
    function Remove-SetupLeftovers { }
    function New-Item { }
    function Set-ItemProperty { param($LiteralPath, $Name, $Value, $Type) $metadata[$Name] = $Value }
    function Remove-ItemProperty { param($LiteralPath, $Name) $metadata.Remove($Name) }
    function Set-StartupMode { throw 'Injected startup failure.' }
    $Drivers = $false; $PreserveExisting = $true; $PreserveDriverOwnership = $true
    $UserSid = 'S-1-5-21-1-2-3-1002'
    foreach ($owner in $null, 0, 1) {
        $metadata = @{ InstallKind = 'zip'; InstalledUserSid = 'S-1-5-21-1-2-3-1001' }
        if ($null -ne $owner) { $metadata.DirectBluetoothDrivers = $owner }
        $before = $metadata.Clone()
        $failure = $null
        try { & $branches.install } catch { $failure = $_ }
        if (-not $failure -or $failure.ToString() -notmatch 'Injected startup failure') { throw 'The install did not reach the injected failure.' }
        & $branches.rollback
        if ($metadata.Count -ne $before.Count) { throw 'Rollback changed metadata presence.' }
        foreach ($name in $before.Keys) {
            if ($metadata[$name] -cne $before[$name]) { throw "Rollback did not preserve $name." }
        }
    }
    function Assert-IdentityRemovalTransaction { $true }
    function Remove-IdentityRemoval { }
    function Get-ItemProperty { throw 'MSI already removed its registry component.' }
    function Remove-RunValues { param($sid, $names) $script:removedStartupSid = $sid }
    $script:removedStartupSid = $null
    & $branches['finish-remove']
    if ($script:removedStartupSid -cne $UserSid) { throw 'Uninstall lost its captured startup owner.' }
}
Write-Host 'PASS failed app-only ZIP conversion preserves the owner, install kind and missing/disabled/enabled driver ownership'
