[CmdletBinding()]
param(
    [ValidateSet('Status', 'Enable', 'Restore', 'Install', 'Rollback', 'Commit')] [string]$Action = 'Status',
    [string]$TransactionId,
    [switch]$SelfTest
)
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSEdition -eq 'Desktop') { Add-Type -AssemblyName System.Security }
$setup = Join-Path $PSScriptRoot 'installer/setup-common.ps1'
if (-not (Test-Path -LiteralPath $setup)) { $setup = Join-Path $PSScriptRoot 'setup-common.ps1' }
. $setup
$script:IdentityKey = 'SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters'
$script:IdentityNames = @('DIDVendorID', 'DIDVendorIDSource')
$script:IdentityTargets = @(
    [pscustomobject]@{ Exists = $true; Kind = 'DWord'; Data = '76' },
    [pscustomobject]@{ Exists = $true; Kind = 'DWord'; Data = '1' }
)
$script:IdentityEntropy = [Text.Encoding]::UTF8.GetBytes('NTPods Bluetooth host identity backup v1')

function Get-IdentityDirectory {
    Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'NTPods\bluetooth-host-identity'
}
function Get-IdentityValue([string]$name) {
    if ($name -notin $script:IdentityNames) { throw 'Unexpected identity value.' }
    if ($name -notin $script:IdentityRegistry.GetValueNames()) {
        return [pscustomobject]@{ Exists = $false; Kind = ''; Data = $null }
    }
    $kind = $script:IdentityRegistry.GetValueKind($name).ToString()
    $data = $script:IdentityRegistry.GetValue($name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    if ($kind -in 'Binary', 'None') { $data = [Convert]::ToBase64String($data) }
    elseif ($kind -in 'DWord', 'QWord') { $data = $data.ToString([Globalization.CultureInfo]::InvariantCulture) }
    [pscustomobject]@{ Exists = $true; Kind = $kind; Data = $data }
}
function Set-IdentityValue([string]$name, $value) {
    if ($name -notin $script:IdentityNames) { throw 'Unexpected identity value.' }
    if (-not $value.Exists) { $script:IdentityRegistry.DeleteValue($name, $false); return }
    $data = switch ($value.Kind) {
        'DWord' { [int]::Parse($value.Data, [Globalization.CultureInfo]::InvariantCulture) }
        'QWord' { [long]::Parse($value.Data, [Globalization.CultureInfo]::InvariantCulture) }
        { $_ -in 'Binary', 'None' } { ,([Convert]::FromBase64String($value.Data)) }
        'MultiString' { ,([string[]]@($value.Data)) }
        default { $value.Data }
    }
    $script:IdentityRegistry.SetValue($name, $data, [Microsoft.Win32.RegistryValueKind]$value.Kind)
    $script:IdentityRegistry.Flush()
}
function Test-IdentityEqual($left, $right) {
    ($left | ConvertTo-Json -Compress -Depth 4) -ceq ($right | ConvertTo-Json -Compress -Depth 4)
}
function Assert-IdentityProperties($value, [string[]]$names) {
    if ($null -eq $value -or @($value.PSObject.Properties.Name).Count -ne $names.Count -or
        @(Compare-Object @($value.PSObject.Properties.Name) $names).Count) { throw 'Invalid identity backup fields.' }
}
function Assert-IdentityBackup($backup) {
    Assert-IdentityProperties $backup @('Version', 'Key', 'Original')
    if ($backup.Version -ne 1 -or $backup.Key -cne $script:IdentityKey) { throw 'Invalid identity backup target.' }
    Assert-IdentityProperties $backup.Original $script:IdentityNames
    foreach ($name in $script:IdentityNames) {
        $value = $backup.Original.$name
        Assert-IdentityProperties $value @('Exists', 'Kind', 'Data')
        if ($value.Exists -isnot [bool]) { throw 'Invalid identity backup presence.' }
        if (-not $value.Exists) {
            if ($value.Kind -cne '' -or $null -ne $value.Data) { throw 'Invalid absent identity value.' }
            continue
        }
        if ($value.Kind -eq 'MultiString') {
            if ($value.Data -isnot [array] -or @($value.Data | Where-Object { $_ -isnot [string] }).Count) { throw 'Invalid identity string array.' }
        } else {
            if ($value.Data -isnot [string]) { throw 'Invalid identity value data.' }
            switch -CaseSensitive ($value.Kind) {
                'DWord' { $null = [int]::Parse($value.Data, [Globalization.CultureInfo]::InvariantCulture) }
                'QWord' { $null = [long]::Parse($value.Data, [Globalization.CultureInfo]::InvariantCulture) }
                { $_ -cin @('Binary', 'None') } { $null = [Convert]::FromBase64String($value.Data) }
                { $_ -cin @('String', 'ExpandString') } { }
                default { throw 'Unsupported identity registry type.' }
            }
        }
    }
}
function Protect-IdentityBackup($backup) {
    Assert-IdentityBackup $backup
    $bytes = [Text.Encoding]::UTF8.GetBytes(($backup | ConvertTo-Json -Compress -Depth 5))
    if ($bytes.Length -gt 1MB) { throw 'Identity backup is too large.' }
    $protected = [Security.Cryptography.ProtectedData]::Protect($bytes, $script:IdentityEntropy, [Security.Cryptography.DataProtectionScope]::LocalMachine)
    if ($protected.Length -gt 1MB) { throw 'Identity backup is too large.' }
    $protected
}
function Unprotect-IdentityBackup([byte[]]$bytes) {
    if ($bytes.Length -gt 1MB) { throw 'Identity backup is too large.' }
    $plain = [Security.Cryptography.ProtectedData]::Unprotect($bytes, $script:IdentityEntropy, [Security.Cryptography.DataProtectionScope]::LocalMachine)
    $backup = [Text.Encoding]::UTF8.GetString($plain) | ConvertFrom-Json
    Assert-IdentityBackup $backup
    $backup
}
function Read-IdentityBackup {
    $path = Join-Path (Get-IdentityDirectory) 'original.bin'
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    $null = Get-InstallTree $path
    if ((Get-Item -LiteralPath $path).Length -gt 1MB) { throw 'Identity backup is too large.' }
    Unprotect-IdentityBackup ([IO.File]::ReadAllBytes($path))
}
function Save-IdentityBackup($backup) {
    $path = Join-Path (Get-IdentityDirectory) 'original.bin'
    $temporary = $path + '.' + [guid]::NewGuid().ToString('N')
    try {
        $stream = [IO.File]::Open($temporary, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $bytes = Protect-IdentityBackup $backup; $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) }
        finally { $stream.Dispose() }
        [IO.File]::Move($temporary, $path)
    } finally {
        if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary }
    }
}
function Remove-IdentityBackup {
    Remove-Item -LiteralPath (Join-Path (Get-IdentityDirectory) 'original.bin')
}
function Assert-IdentityOwned($backup) {
    for ($i = 0; $i -lt $script:IdentityNames.Count; $i++) {
        $name = $script:IdentityNames[$i]
        $current = Get-IdentityValue $name
        if (-not (Test-IdentityEqual $current $script:IdentityTargets[$i]) -and
            -not (Test-IdentityEqual $current $backup.Original.$name)) {
            throw "$name changed outside this helper. Restore refused; the backup is retained."
        }
    }
}
function Restore-Identity($backup) {
    Assert-IdentityBackup $backup
    Assert-IdentityOwned $backup
    foreach ($name in $script:IdentityNames) {
        Assert-IdentityOwned $backup
        if (-not (Test-IdentityEqual (Get-IdentityValue $name) $backup.Original.$name)) {
            Set-IdentityValue $name $backup.Original.$name
        }
    }
    foreach ($name in $script:IdentityNames) {
        if (-not (Test-IdentityEqual (Get-IdentityValue $name) $backup.Original.$name)) { throw 'Identity restoration did not persist; backup retained.' }
    }
    Remove-IdentityBackup
}
function Read-IdentityTransaction {
    $path = Join-Path (Get-IdentityDirectory) 'transaction.json'
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    $null = Get-InstallTree $path
    if ((Get-Item -LiteralPath $path).Length -gt 4096) { throw 'Invalid identity transaction size.' }
    $value = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    Assert-IdentityProperties $value @('TransactionId', 'CreatedBackup')
    if ($value.TransactionId -isnot [string] -or [string]::IsNullOrEmpty($value.TransactionId) -or
        $value.CreatedBackup -isnot [bool]) { throw 'Invalid identity transaction.' }
    $value
}
function Save-IdentityTransaction($value) {
    $path = Join-Path (Get-IdentityDirectory) 'transaction.json'
    $temporary = $path + '.' + [guid]::NewGuid().ToString('N')
    try {
        $stream = [IO.File]::Open($temporary, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try {
            $bytes = [Text.Encoding]::UTF8.GetBytes(($value | ConvertTo-Json -Compress))
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush($true)
        } finally { $stream.Dispose() }
        [IO.File]::Move($temporary, $path)
    } finally {
        if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary }
    }
}
function Remove-IdentityTransaction {
    Remove-Item -LiteralPath (Join-Path (Get-IdentityDirectory) 'transaction.json')
}
function Invoke-IdentityTransaction([string]$action, [string]$transactionId) {
    if ([string]::IsNullOrEmpty($transactionId) -or $transactionId.Length -gt 512) { throw 'An identity transaction ID is required.' }
    $saved = Read-IdentityTransaction
    if ($saved -and $saved.TransactionId -cne $transactionId) { throw 'An earlier identity transaction still needs recovery.' }
    if ($action -eq 'Install') {
        if ($saved) { throw 'This identity transaction already started; roll it back before retrying.' }
        $backup = Read-IdentityBackup
        if ($backup) {
            Assert-IdentityBackup $backup
            # A partial manual restore must finish before an installer takes ownership.
            for ($i = 0; $i -lt $script:IdentityNames.Count; $i++) {
                if (-not (Test-IdentityEqual (Get-IdentityValue $script:IdentityNames[$i]) $script:IdentityTargets[$i])) {
                    throw 'The saved Bluetooth identity needs Restore before installation can continue.'
                }
            }
        }
        Save-IdentityTransaction ([pscustomobject]@{ TransactionId = $transactionId; CreatedBackup = $null -eq $backup })
        Invoke-IdentityChange 'Enable'
    } elseif ($saved) {
        if ($action -eq 'Rollback' -and $saved.CreatedBackup) { Invoke-IdentityChange 'Restore' }
        Remove-IdentityTransaction
    }
}
function Invoke-IdentityChange([string]$action) {
    $backup = Read-IdentityBackup
    if ($action -eq 'Restore') {
        if ($null -eq $backup) { return 'No saved identity change to restore.' }
        Restore-Identity $backup
        return 'Original Bluetooth host identity restored.'
    }
    if ($null -eq $backup) {
        $original = [ordered]@{}
        foreach ($name in $script:IdentityNames) { $original[$name] = Get-IdentityValue $name }
        $backup = [pscustomobject]@{ Version = 1; Key = $script:IdentityKey; Original = [pscustomobject]$original }
        Save-IdentityBackup $backup
    }
    Assert-IdentityBackup $backup
    Assert-IdentityOwned $backup
    try {
        for ($i = 0; $i -lt $script:IdentityNames.Count; $i++) {
            Assert-IdentityOwned $backup
            $name = $script:IdentityNames[$i]
            if (-not (Test-IdentityEqual (Get-IdentityValue $name) $script:IdentityTargets[$i])) {
                Set-IdentityValue $name $script:IdentityTargets[$i]
            }
        }
        for ($i = 0; $i -lt $script:IdentityNames.Count; $i++) {
            if (-not (Test-IdentityEqual (Get-IdentityValue $script:IdentityNames[$i]) $script:IdentityTargets[$i])) { throw 'Identity change did not persist.' }
        }
    } catch {
        $failure = $_
        try { Restore-Identity $backup }
        catch { throw "Identity change failed and restoration needs attention: $_" }
        throw $failure
    }
    'Apple Bluetooth host identity configured. The original values are saved for Restore.'
}

if ($MyInvocation.InvocationName -eq '.') { return }
if ($SelfTest) {
    $test = Join-Path $PSScriptRoot 'tests/test-bluetooth-host-identity.ps1'
    if (-not (Test-Path -LiteralPath $test)) { throw 'SelfTest requires the source checkout; tests are not included in releases.' }
    & $test
    return
}
if ($Action -ne 'Status' -and -not (Test-Admin)) { throw 'Run Enable or Restore from an Administrator PowerShell.' }
$base = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, [Microsoft.Win32.RegistryView]::Registry64)
$lock = $null
try {
    $script:IdentityRegistry = $base.OpenSubKey($script:IdentityKey, ($Action -ne 'Status'))
    if ($null -eq $script:IdentityRegistry) { throw 'Windows Bluetooth parameters were not found.' }
    if ($Action -eq 'Status') {
        foreach ($name in $script:IdentityNames) { [pscustomobject]@{ Name = $name; Value = (Get-IdentityValue $name) } }
        return
    }
    Write-Host 'This changes the Bluetooth identity of this Windows PC for all remote devices.'
    $directory = Get-IdentityDirectory
    foreach ($path in @([IO.Path]::GetDirectoryName($directory), $directory)) {
        $acl = [Security.AccessControl.DirectorySecurity]::new()
        $acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)')
        New-DirectoryWithSecurity $path $acl
        $entry = Get-Item -LiteralPath $path -Force
        if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Identity backup path is a reparse point.' }
        Assert-TrustedInstallEntry $path
    }
    $lockPath = Join-Path $directory 'operation.lock'
    if (Test-Path -LiteralPath $lockPath) { $null = Get-InstallTree $lockPath }
    $lock = [IO.File]::Open($lockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    if ($Action -in 'Install', 'Rollback', 'Commit') { Invoke-IdentityTransaction $Action $TransactionId }
    else {
        if (Read-IdentityTransaction) { throw 'An installer identity transaction still needs recovery.' }
        Invoke-IdentityChange $Action
    }
    Write-Host 'No radio was restarted or device re-paired. Restart Windows before checking the advertised identity.'
} finally {
    if ($lock) { $lock.Dispose() }
    if ($script:IdentityRegistry) { $script:IdentityRegistry.Dispose() }
    $base.Dispose()
}
