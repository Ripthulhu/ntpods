# Real temporary files and the ZIP install body; no drivers, registry or tasks change.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'setup-common.ps1')
. (Join-Path $PSScriptRoot 'app-transaction.ps1')
$testRoot = [IO.Path]::GetFullPath((Join-Path $env:TEMP ('ntpods-app-transaction-' + [guid]::NewGuid())))
$tempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')
if (-not $testRoot.StartsWith($tempRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe test directory.' }
$savedProgramFiles = $env:ProgramFiles
$script:heldFile = $null
function Assert-True($condition, [string]$message) { if (-not $condition) { throw $message } }
function Assert-Fails([scriptblock]$body) {
    try { & $body } catch { return $_ }
    throw 'Expected failure.'
}

# ACL installation needs elevation; retain the real tree and reparse-point validation.
function Assert-TrustedInstallEntry([string]$path) {
    if (-not [IO.Path]::GetFullPath($path).StartsWith($testRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Path escaped the fixture.' }
}
function New-DirectoryWithSecurity([string]$path, $acl) { [IO.Directory]::CreateDirectory($path) | Out-Null }
function Assert-ProtectedInstallPath([string]$path) {
    if ($path -ine (Join-Path $env:ProgramFiles 'NTPods')) { throw 'Wrong install destination.' }
    @(Get-InstallTree $path) | Out-Null
}
function Install-NTPodsDrivers { $script:driverInstalled = $true }
function Restore-NTPodsDriverCheckpoint {
    $script:driverRestored = $true
    if ($script:failDriverRecovery) { throw 'Injected driver recovery failure.' }
}
function Get-DriverCheckpointPath { Join-Path $testRoot 'driver-backup' }
function Register-NTPodsTasks([string]$dest) {
    if ($script:failAt -eq 'rollback') {
        $script:heldFile = [IO.File]::Open((Join-Path $dest 'ntpodsd.exe'), 'Open', 'Read', 'Read')
    }
    if ($script:failAt -in 'later', 'rollback') { throw 'Injected task registration failure.' }
}

$tokens = $null; $parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'install.ps1'), [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw 'Invalid ZIP setup script.' }
$install = $ast.EndBlock.Statements | Where-Object {
    $_ -is [Management.Automation.Language.TryStatementAst] -and $_.Body.Extent.Text -match 'Prepare-NTPodsAppTransaction'
}
Assert-True (@($install).Count -eq 1) 'The test must execute the ZIP app transaction and its recovery.'
$installBody = [scriptblock]::Create($install.Extent.Text)

function Reset-Fixture([string]$name, [bool]$existing) {
    $script:caseRoot = Join-Path $testRoot $name
    $env:ProgramFiles = Join-Path $script:caseRoot 'Program Files'
    $script:source = Join-Path $script:caseRoot 'release'
    [IO.Directory]::CreateDirectory($env:ProgramFiles) | Out-Null
    [IO.Directory]::CreateDirectory($script:source) | Out-Null
    $script:destination = Join-Path $env:ProgramFiles 'NTPods'
    foreach ($name in 'ntpodsd.exe', 'avcodec-61.dll', 'avutil-59.dll', 'swresample-5.dll', 'ntpods-aac-encoder.exe', 'ntpods-eld-encoder.exe', 'fdk-aac.dll', 'fix-driver.ps1', 'rename-mic.ps1', 'setup-common.ps1', 'audio-driver.ps1', 'restore-playback.ps1', 'stop-ntpods.ps1', 'msi-setup.ps1', 'uninstall.ps1') {
        [IO.File]::WriteAllText((Join-Path $script:source $name), "new:$name")
    }
    [IO.File]::WriteAllText((Join-Path $script:source 'bluetooth-host-identity.ps1'), 'param($Action, $TransactionId)')
    foreach ($name in 'winui', 'tools', 'licenses') {
        [IO.Directory]::CreateDirectory((Join-Path $script:source $name)) | Out-Null
        [IO.File]::WriteAllText((Join-Path $script:source "$name\payload.txt"), "new:$name")
    }
    if ($existing) {
        [IO.Directory]::CreateDirectory((Join-Path $script:destination 'winui')) | Out-Null
        [IO.File]::WriteAllText((Join-Path $script:destination 'ntpodsd.exe'), 'old:daemon')
        [IO.File]::WriteAllText((Join-Path $script:destination 'avcodec-61.dll'), 'old:library')
        [IO.File]::WriteAllText((Join-Path $script:destination 'winui\payload.txt'), 'old:ui')
        [IO.File]::WriteAllText((Join-Path $script:destination 'preserved.txt'), 'old:other-file')
    }
    $script:driverInstalled = $false
    $script:driverRestored = $false
    $script:failDriverRecovery = $false
    $script:failAt = 'later'
}

function Invoke-InstallFixture([string]$transactionId) {
    $here = $script:source
    $dest = $script:destination
    $setupDir = $script:caseRoot
    $work = Join-Path $setupDir 'driver-work'
    $sid = 'S-1-5-21-1-2-3-1001'
    $MicDriver = 'acx'
    & $installBody
}

function Assert-Original {
    Assert-True ([IO.File]::ReadAllText((Join-Path $script:destination 'ntpodsd.exe')) -ceq 'old:daemon') 'Old daemon was not restored.'
    Assert-True ([IO.File]::ReadAllText((Join-Path $script:destination 'avcodec-61.dll')) -ceq 'old:library') 'Old library was not restored.'
    Assert-True ([IO.File]::ReadAllText((Join-Path $script:destination 'winui\payload.txt')) -ceq 'old:ui') 'Old UI was not restored.'
    Assert-True ([IO.File]::ReadAllText((Join-Path $script:destination 'preserved.txt')) -ceq 'old:other-file') 'Unreplaced files were lost.'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $script:destination 'fdk-aac.dll'))) 'Failed upgrade kept a new file.'
}

try {
    Reset-Fixture 'copy-failure' $true
    $script:heldFile = [IO.File]::Open((Join-Path $script:source 'avcodec-61.dll'), 'Open', 'Read', 'None')
    try { $failure = Assert-Fails { Invoke-InstallFixture 'copy' } }
    finally { $script:heldFile.Dispose(); $script:heldFile = $null }
    Assert-True ($failure.ToString() -match 'avcodec-61.dll') 'The staged copy did not fail at the locked second file.'
    Assert-Original
    Assert-True (-not $script:driverInstalled) 'Staging failure reached driver installation.'
    Assert-True (-not (Test-Path -LiteralPath (Get-NTPodsAppTransactionPath))) 'Staging failure left a transaction.'

    Reset-Fixture 'later-failure' $true
    $failure = Assert-Fails { Invoke-InstallFixture 'later' }
    Assert-True ($failure.ToString() -match 'Injected task registration failure') 'The later setup failure was not reached.'
    Assert-Original
    Assert-True ($script:driverInstalled -and $script:driverRestored) 'The failed upgrade did not restore its drivers.'
    Assert-True (-not (Test-Path -LiteralPath (Get-NTPodsAppTransactionPath))) 'Successful recovery left its backup.'
    Restore-NTPodsAppTransaction 'later'
    Assert-Original

    Reset-Fixture 'driver-recovery-failure' $true
    $script:failDriverRecovery = $true
    $failure = Assert-Fails { Invoke-InstallFixture 'driver-recovery' }
    Assert-True ($failure.ToString() -match 'Driver recovery failed') 'Driver recovery failure was not reported.'
    Assert-Original

    Reset-Fixture 'first-install-failure' $false
    $null = Assert-Fails { Invoke-InstallFixture 'first' }
    Assert-True (-not (Test-Path -LiteralPath $script:destination)) 'Failed first install left application files.'
    Assert-True (-not (Test-Path -LiteralPath (Get-NTPodsAppTransactionPath))) 'Failed first install left its transaction.'

    Reset-Fixture 'rollback-failure' $true
    $script:failAt = 'rollback'
    try { $failure = Assert-Fails { Invoke-InstallFixture 'recovery' } }
    finally { $script:heldFile.Dispose(); $script:heldFile = $null }
    Assert-True ($failure.ToString() -match 'App recovery failed') 'A failed recovery was not reported.'
    $previous = Join-Path (Get-NTPodsAppTransactionPath) 'previous\ntpodsd.exe'
    Assert-True ([IO.File]::ReadAllText($previous) -ceq 'old:daemon') 'Failed recovery lost the original backup.'
    $null = Assert-Fails { Complete-NTPodsAppTransaction 'other' }
    Assert-True (Test-Path -LiteralPath $previous) 'A mismatched transaction removed the backup.'
    Restore-NTPodsAppTransaction 'recovery'
    Assert-Original

    Reset-Fixture 'source-junction' $true
    $outside = Join-Path $script:caseRoot 'outside'
    [IO.Directory]::CreateDirectory($outside) | Out-Null
    [IO.File]::WriteAllText((Join-Path $outside 'untouched.txt'), 'untouched')
    $junction = Join-Path $script:source 'winui\linked'
    New-Item -ItemType Junction -Path $junction -Target $outside | Out-Null
    try {
        $failure = Assert-Fails { Invoke-InstallFixture 'source-junction' }
        Assert-True ($failure.ToString() -match 'reparse point') 'The source junction was followed.'
        Assert-Original
        Assert-True ([IO.File]::ReadAllText((Join-Path $outside 'untouched.txt')) -ceq 'untouched') 'The junction target changed.'
    } finally { [IO.Directory]::Delete($junction) }

    Reset-Fixture 'backup-junction' $true
    $outside = Join-Path $script:caseRoot 'outside'
    [IO.Directory]::CreateDirectory($outside) | Out-Null
    [IO.File]::WriteAllText((Join-Path $outside 'untouched.txt'), 'untouched')
    $junction = Get-NTPodsAppTransactionPath
    New-Item -ItemType Junction -Path $junction -Target $outside | Out-Null
    try {
        $null = Assert-Fails { Prepare-NTPodsAppTransaction $script:source 'backup-junction' }
        $null = Assert-Fails { Restore-NTPodsAppTransaction 'backup-junction' }
        Assert-Original
        Assert-True ([IO.File]::ReadAllText((Join-Path $outside 'untouched.txt')) -ceq 'untouched') 'Recovery followed a backup junction.'
    } finally { [IO.Directory]::Delete($junction) }

    Reset-Fixture 'commit' $true
    Prepare-NTPodsAppTransaction $script:source 'commit'
    Install-NTPodsAppTransaction 'commit'
    Assert-True ([IO.File]::ReadAllText((Join-Path $script:destination 'ntpodsd.exe')) -ceq 'new:ntpodsd.exe') 'The new app was not activated.'
    Assert-True ([IO.File]::ReadAllText((Join-Path $script:destination 'preserved.txt')) -ceq 'old:other-file') 'Successful upgrade discarded unrelated installed files.'
    Complete-NTPodsAppTransaction 'commit'
    Assert-True (-not (Test-Path -LiteralPath (Get-NTPodsAppTransactionPath))) 'Commit retained its backup.'
    Write-Host 'PASS ZIP app staging, failed upgrade and fresh install, retained recovery backup, transaction ownership, reparse rejection and commit'
} finally {
    if ($script:heldFile) { $script:heldFile.Dispose() }
    $env:ProgramFiles = $savedProgramFiles
    if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
