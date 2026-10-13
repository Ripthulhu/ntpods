$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '../bluetooth-host-identity.ps1')

function Assert-True($condition, [string]$message) { if (-not $condition) { throw $message } }
function Assert-Fails([scriptblock]$body, [string]$message) {
    $failed = $false
    try { & $body | Out-Null } catch { $failed = $true }
    Assert-True $failed $message
}
function New-Value([string]$kind, $data) {
    [pscustomobject]@{ Exists = $true; Kind = $kind; Data = $data }
}
class IdentityRegistryFixture {
    [hashtable]$Values = @{}
    [string[]] GetValueNames() { return @($this.Values.Keys) }
    [Microsoft.Win32.RegistryValueKind] GetValueKind([string]$name) { return $this.Values[$name].Kind }
    [object] GetValue([string]$name, [object]$fallback, [Microsoft.Win32.RegistryValueOptions]$options) { return $this.Values[$name].Data }
    [void] SetValue([string]$name, [object]$data, [Microsoft.Win32.RegistryValueKind]$kind) { $this.Values[$name] = @{ Kind = $kind; Data = $data } }
    [void] DeleteValue([string]$name, [bool]$throwOnMissing) { $this.Values.Remove($name) }
    [void] Flush() { }
}
$script:IdentityRegistry = [IdentityRegistryFixture]::new()
foreach ($value in @(
    (New-Value 'String' 'unchanged'), (New-Value 'ExpandString' '%SystemRoot%'),
    (New-Value 'MultiString' @('one', 'two')), (New-Value 'MultiString' @('one')), (New-Value 'MultiString' @()),
    (New-Value 'Binary' 'AP+A'), (New-Value 'None' ''),
    (New-Value 'DWord' '-2147483648'), (New-Value 'QWord' '-9223372036854775808')
)) {
    Set-IdentityValue 'DIDVendorID' $value
    Assert-True (Test-IdentityEqual (Get-IdentityValue 'DIDVendorID') $value) "Registry conversion lost $($value.Kind) data."
}
Set-IdentityValue 'DIDVendorID' ([pscustomobject]@{ Exists = $false; Kind = ''; Data = $null })
Assert-True (-not (Get-IdentityValue 'DIDVendorID').Exists) 'Registry value deletion failed.'

function Reset-IdentityFixture {
    $script:Memory = @{
        DIDVendorID = [pscustomobject]@{ Exists = $false; Kind = ''; Data = $null }
        DIDVendorIDSource = New-Value 'DWord' '2'
    }
    $script:Saved = $null
    $script:Writes = 0
    $script:Saves = 0
    $script:FailWrite = 0
    $script:FailAfterWrite = 0
    $script:FailSave = $false
    $script:FailRemove = $false
    $script:Transaction = $null
    $script:FailTransactionSave = $false
}
function Get-IdentityValue([string]$name) { $script:Memory[$name] }
function Set-IdentityValue([string]$name, $value) {
    $script:Writes++
    if ($script:Writes -eq $script:FailWrite) { throw 'Injected registry failure.' }
    $script:Memory[$name] = $value
    if ($script:Writes -eq $script:FailAfterWrite) { throw 'Injected failure after registry write.' }
}
function Read-IdentityBackup {
    if ($null -ne $script:Saved) { Unprotect-IdentityBackup $script:Saved }
}
function Save-IdentityBackup($backup) {
    if ($script:FailSave -or $null -ne $script:Saved) { throw 'Injected backup failure.' }
    $script:Saves++
    $script:Saved = [byte[]](Protect-IdentityBackup $backup)
}
function Remove-IdentityBackup {
    if ($script:FailRemove) { throw 'Injected cleanup failure.' }
    $script:Saved = $null
}
function Read-IdentityTransaction { $script:Transaction }
function Save-IdentityTransaction($value) {
    if ($script:FailTransactionSave -or $script:Transaction) { throw 'Injected transaction save failure.' }
    $script:Transaction = $value
}
function Remove-IdentityTransaction { $script:Transaction = $null }

Reset-IdentityFixture
$null = Invoke-IdentityChange 'Enable'
Assert-True ($script:Memory.DIDVendorID.Data -eq '76' -and $script:Memory.DIDVendorIDSource.Data -eq '1') 'Apple identity not configured.'
$encodedSnapshot = [Convert]::ToBase64String($script:Saved)
$null = Invoke-IdentityChange 'Enable'
Assert-True ($script:Writes -eq 2 -and $script:Saves -eq 1 -and [Convert]::ToBase64String($script:Saved) -ceq $encodedSnapshot) 'Repeated Enable replaced the original backup.'
$null = Invoke-IdentityChange 'Restore'
Assert-True (-not $script:Memory.DIDVendorID.Exists -and $script:Memory.DIDVendorIDSource.Data -eq '2' -and $null -eq $script:Saved) 'Original presence and value not restored.'
$writes = $script:Writes
$null = Invoke-IdentityChange 'Restore'
Assert-True ($script:Writes -eq $writes) 'Restore without a backup wrote values.'

foreach ($value in @(
    (New-Value 'String' 'unchanged'), (New-Value 'ExpandString' '%SystemRoot%'),
    (New-Value 'MultiString' @('one', 'two')), (New-Value 'MultiString' @()),
    (New-Value 'Binary' 'AP+A'), (New-Value 'None' ''),
    (New-Value 'DWord' '-2147483648'), (New-Value 'QWord' '-9223372036854775808')
)) {
    Reset-IdentityFixture
    $script:Memory.DIDVendorID = $value
    $null = Invoke-IdentityChange 'Enable'
    $null = Invoke-IdentityChange 'Restore'
    Assert-True (Test-IdentityEqual $script:Memory.DIDVendorID $value) "Original $($value.Kind) data changed."
}

Reset-IdentityFixture
$script:FailSave = $true
Assert-Fails { Invoke-IdentityChange 'Enable' } 'Enable ignored backup failure.'
Assert-True ($script:Writes -eq 0) 'Registry changed before backup was saved.'

Reset-IdentityFixture
$script:FailWrite = 2
Assert-Fails { Invoke-IdentityChange 'Enable' } 'Enable ignored partial write failure.'
Assert-True (-not $script:Memory.DIDVendorID.Exists -and $script:Memory.DIDVendorIDSource.Data -eq '2' -and $null -eq $script:Saved) 'Partial Enable did not roll back.'

Reset-IdentityFixture
$script:FailAfterWrite = 2
Assert-Fails { Invoke-IdentityChange 'Enable' } 'Enable ignored failure after a write.'
Assert-True (-not $script:Memory.DIDVendorID.Exists -and $script:Memory.DIDVendorIDSource.Data -eq '2' -and $null -eq $script:Saved) 'Completed writes did not roll back after failure.'

Reset-IdentityFixture
$null = Invoke-IdentityChange 'Enable'
$script:FailWrite = 4
Assert-Fails { Invoke-IdentityChange 'Restore' } 'Restore ignored partial write failure.'
Assert-True ($null -ne $script:Saved) 'Partial Restore discarded the backup.'
$null = Invoke-IdentityChange 'Restore'
Assert-True (-not $script:Memory.DIDVendorID.Exists -and $script:Memory.DIDVendorIDSource.Data -eq '2') 'Retry did not finish Restore.'

Reset-IdentityFixture
$null = Invoke-IdentityChange 'Enable'
$script:Memory.DIDVendorIDSource = New-Value 'DWord' '99'
$writes = $script:Writes
Assert-Fails { Invoke-IdentityChange 'Restore' } 'Restore overwrote a third-party change.'
Assert-Fails { Invoke-IdentityChange 'Enable' } 'Enable overwrote a third-party change.'
Assert-True ($script:Writes -eq $writes -and $script:Memory.DIDVendorIDSource.Data -eq '99' -and $null -ne $script:Saved) 'Conflicting values were changed or backup lost.'

Reset-IdentityFixture
$null = Invoke-IdentityChange 'Enable'
$script:Saved[30] = $script:Saved[30] -bxor 1
$writes = $script:Writes
Assert-Fails { Invoke-IdentityChange 'Restore' } 'Modified DPAPI backup accepted.'
Assert-True ($script:Writes -eq $writes) 'Modified backup caused registry writes.'

Reset-IdentityFixture
$null = Invoke-IdentityChange 'Enable'
$backup = Read-IdentityBackup
$backup.Key = 'SOFTWARE\Unrelated'
Assert-Fails { Protect-IdentityBackup $backup } 'Unexpected registry path accepted.'
$backup.Key = $script:IdentityKey
$backup.Original.DIDVendorIDSource.Data = '4294967296'
Assert-Fails { Protect-IdentityBackup $backup } 'Out-of-range DWORD accepted.'
$backup.Original.DIDVendorIDSource.Data = '2'
$backup.Original | Add-Member Unexpected (New-Value 'DWord' '1')
Assert-Fails { Protect-IdentityBackup $backup } 'Unexpected registry value accepted.'

Reset-IdentityFixture
$script:Memory.DIDVendorID = New-Value 'String' ('x' * 1MB)
Assert-Fails { Invoke-IdentityChange 'Enable' } 'Oversized backup accepted.'
Assert-True ($script:Writes -eq 0) 'Oversized backup caused registry writes.'

Reset-IdentityFixture
$null = Invoke-IdentityChange 'Enable'
$script:FailRemove = $true
Assert-Fails { Invoke-IdentityChange 'Restore' } 'Failed backup removal was ignored.'
$script:FailRemove = $false
$null = Invoke-IdentityChange 'Restore'
Assert-True ($null -eq $script:Saved) 'Completed restoration could not be retried.'

foreach ($existing in $false, $true) {
    Reset-IdentityFixture
    if ($existing) { $null = Invoke-IdentityChange 'Enable' }
    $before = $script:Saved
    $null = Invoke-IdentityTransaction 'Install' 'upgrade-1'
    Assert-True ($script:Transaction.CreatedBackup -eq (-not $existing)) 'Installer lost backup ownership.'
    $writes = $script:Writes
    Assert-Fails { Invoke-IdentityTransaction 'Rollback' 'other-upgrade' } 'Another installer rolled back this transaction.'
    Assert-Fails { Invoke-IdentityTransaction 'Commit' 'other-upgrade' } 'Another installer committed this transaction.'
    Assert-True ($script:Writes -eq $writes) 'Wrong transaction changed registry values.'
    $null = Invoke-IdentityTransaction 'Rollback' 'upgrade-1'
    if ($existing) {
        Assert-True ($script:Memory.DIDVendorID.Data -eq '76' -and
            [Convert]::ToBase64String($script:Saved) -ceq [Convert]::ToBase64String($before)) 'Failed upgrade restored or replaced the existing identity backup.'
    } else {
        Assert-True (-not $script:Memory.DIDVendorID.Exists -and $script:Memory.DIDVendorIDSource.Data -eq '2' -and
            $null -eq $script:Saved) 'Failed initial setup did not restore the original identity.'
    }
    Assert-True ($null -eq $script:Transaction) 'Successful installer rollback left its journal.'
    $writes = $script:Writes
    $null = Invoke-IdentityTransaction 'Rollback' 'upgrade-1'
    Assert-True ($script:Writes -eq $writes) 'Repeated installer rollback wrote to the registry.'
}

Reset-IdentityFixture
$null = Invoke-IdentityTransaction 'Install' 'fresh'
$original = [Convert]::ToBase64String($script:Saved)
$null = Invoke-IdentityTransaction 'Commit' 'fresh'
Assert-True ($null -eq $script:Transaction -and [Convert]::ToBase64String($script:Saved) -ceq $original) 'Commit removed the original identity backup.'
$null = Invoke-IdentityTransaction 'Install' 'repair'
$null = Invoke-IdentityTransaction 'Commit' 'repair'
Assert-True ([Convert]::ToBase64String($script:Saved) -ceq $original) 'Repair replaced the original backup.'
$null = Invoke-IdentityChange 'Restore'
Assert-True (-not $script:Memory.DIDVendorID.Exists -and $script:Memory.DIDVendorIDSource.Data -eq '2') 'Uninstall after repair did not restore the original identity.'

Reset-IdentityFixture
$script:FailTransactionSave = $true
Assert-Fails { Invoke-IdentityTransaction 'Install' 'fresh' } 'Install ignored journal creation failure.'
Assert-True ($script:Writes -eq 0 -and $null -eq $script:Saved) 'Install changed identity before saving its journal.'

Reset-IdentityFixture
$script:FailWrite = 2
Assert-Fails { Invoke-IdentityTransaction 'Install' 'fresh' } 'Install ignored Enable failure.'
$null = Invoke-IdentityTransaction 'Rollback' 'fresh'
Assert-True ($null -eq $script:Saved -and $null -eq $script:Transaction -and
    -not $script:Memory.DIDVendorID.Exists) 'Enable failure left an installer change or journal.'

Reset-IdentityFixture
$null = Invoke-IdentityChange 'Enable'
$script:Memory.DIDVendorID = [pscustomobject]@{ Exists = $false; Kind = ''; Data = $null }
$writes = $script:Writes
Assert-Fails { Invoke-IdentityTransaction 'Install' 'repair' } 'Installer claimed a partially restored identity.'
Assert-True ($script:Writes -eq $writes -and $null -eq $script:Transaction) 'Incomplete manual restoration was changed by installation.'

Reset-IdentityFixture
$null = Invoke-IdentityTransaction 'Install' 'fresh'
$script:Memory.DIDVendorIDSource = New-Value 'DWord' '99'
Assert-Fails { Invoke-IdentityTransaction 'Rollback' 'fresh' } 'Installer rollback overwrote a third-party change.'
Assert-True ($null -ne $script:Transaction -and $null -ne $script:Saved -and
    $script:Memory.DIDVendorIDSource.Data -eq '99') 'Failed recovery discarded evidence or changed the conflicting value.'

$originalProgramData = $env:ProgramData
$removalFixture = Join-Path $env:TEMP ('ntpods-identity-removal-' + [guid]::NewGuid().ToString('N'))
$trustedTree = (Get-Item Function:Get-InstallTree).ScriptBlock
try {
    $env:ProgramData = $removalFixture
    $source = Join-Path $removalFixture 'source'
    New-Item -ItemType Directory -Path $source, (Join-Path $removalFixture 'NTPods') | Out-Null
    function Get-InstallTree([string]$path) { Get-Item -LiteralPath $path }
    foreach ($name in 'msi-setup.ps1', 'setup-common.ps1', 'bluetooth-host-identity.ps1', 'stop-ntpods.ps1', 'audio-driver.ps1') {
        Set-Content -LiteralPath (Join-Path $source $name) -Value '# isolated fixture'
    }
    Reset-IdentityFixture
    $null = Invoke-IdentityChange 'Enable'
    $writes = $script:Writes
    Prepare-IdentityRemoval $source 'remove-1'
    Assert-True ($script:Writes -eq $writes -and $null -ne $script:Saved) 'Preparing uninstall changed Bluetooth identity before commit.'
    Assert-True (@(Get-ChildItem -LiteralPath (Get-IdentityRemovalPath) -Filter '*.ps1').Count -eq 5) 'Removal did not stage its complete helper dependencies.'
    Assert-Fails { Prepare-IdentityRemoval $source 'remove-2' } 'Another uninstall replaced pending recovery files.'
    Assert-Fails { Remove-IdentityRemoval 'remove-2' } 'Another transaction deleted pending recovery files.'
    Remove-IdentityRemoval 'remove-1'
    Assert-True ($script:Writes -eq $writes -and $null -ne $script:Saved -and
        -not (Test-Path -LiteralPath (Get-IdentityRemovalPath))) 'Cancelled uninstall changed identity or retained its staging files.'
    Prepare-IdentityRemoval $source 'remove-3'
    Assert-True (Assert-IdentityRemovalTransaction 'remove-3') 'Removal commit could not verify its staged helper.'
    $script:Memory.DIDVendorIDSource = New-Value 'DWord' '99'
    Assert-Fails { Invoke-IdentityChange 'Restore'; Remove-IdentityRemoval 'remove-3' } 'Removal commit overwrote an external identity change.'
    Assert-True ((Test-Path -LiteralPath (Get-IdentityRemovalPath)) -and $null -ne $script:Saved) 'Failed removal commit discarded recovery files.'
    $script:Memory.DIDVendorIDSource = New-Value 'DWord' '1'
    $null = Invoke-IdentityChange 'Restore'
    Remove-IdentityRemoval 'remove-3'
    Assert-True ($null -eq $script:Saved -and -not (Test-Path -LiteralPath (Get-IdentityRemovalPath))) 'Successful removal retained its backup or staged scripts.'
} finally {
    Set-Item Function:Get-InstallTree $trustedTree
    $env:ProgramData = $originalProgramData
    $target = [IO.Path]::GetFullPath($removalFixture)
    if (-not $target.StartsWith([IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Unsafe identity test cleanup path.'
    }
    if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Recurse -Force }
}
Write-Output 'Bluetooth host identity checks passed. No system registry values were changed.'
