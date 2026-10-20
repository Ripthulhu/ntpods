# ZIP upgrades keep the old app until driver and system setup have both succeeded.
function Get-NTPodsAppTransactionPath {
    $parent = [IO.Path]::GetFullPath($env:ProgramFiles).TrimEnd('\')
    if ((Get-Item -LiteralPath $parent -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Program Files cannot be a reparse point.' }
    Assert-TrustedInstallEntry $parent
    Join-Path $parent 'NTPods.setup'
}

function Read-NTPodsAppTransaction([string]$transactionId) {
    if ([string]::IsNullOrEmpty($transactionId)) { throw 'An app transaction ID is required.' }
    $path = Get-NTPodsAppTransactionPath
    if (-not (Test-Path -LiteralPath $path)) { return }
    @(Get-InstallTree $path) | Out-Null
    $file = Join-Path $path 'transaction.json'
    if ((Get-Item -LiteralPath $file).Length -gt 4096) { throw 'Invalid app transaction.' }
    $saved = Get-Content -LiteralPath $file -Raw | ConvertFrom-Json
    if ($saved.TransactionId -cne $transactionId -or $saved.HadInstall -isnot [bool]) {
        throw "The app backup belongs to another transaction and was retained: $path"
    }
    $saved
}

function Copy-NTPodsAppEntry([string]$source, [string]$destination) {
    $entry = Get-Item -LiteralPath $source -Force -ErrorAction Stop
    if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "App source contains a reparse point: $source" }
    if ($entry.PSIsContainer) {
        [IO.Directory]::CreateDirectory($destination) | Out-Null
        foreach ($child in Get-ChildItem -LiteralPath $source -Force) {
            Copy-NTPodsAppEntry $child.FullName (Join-Path $destination $child.Name)
        }
    } else { Copy-Item -LiteralPath $source -Destination $destination -Force -ErrorAction Stop }
}

function Prepare-NTPodsAppTransaction([string]$root, [string]$transactionId) {
    if ([string]::IsNullOrEmpty($transactionId) -or $transactionId.Length -gt 512) { throw 'An app transaction ID is required.' }
    $path = Get-NTPodsAppTransactionPath
    if (Test-Path -LiteralPath $path) { throw "An earlier app backup needs recovery or cleanup: $path" }
    $dest = Join-Path $env:ProgramFiles 'NTPods'
    $hadInstall = Test-Path -LiteralPath $dest
    if ($hadInstall) { Assert-ProtectedInstallPath $dest }
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)')
    New-DirectoryWithSecurity $path $acl
    try {
        @(Get-InstallTree $path) | Out-Null
        [pscustomobject]@{ TransactionId = $transactionId; HadInstall = $hadInstall } |
            ConvertTo-Json | Set-Content -LiteralPath (Join-Path $path 'transaction.json') -Encoding UTF8 -ErrorAction Stop
        $staged = Join-Path $path 'new'
        # Directory renames stay on one volume and retain this readable, protected ACL.
        $acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;BU)')
        New-DirectoryWithSecurity $staged $acl
        if ($hadInstall) { Copy-NTPodsAppEntry $dest $staged }
        foreach ($name in 'ntpodsd.exe', 'avcodec-61.dll', 'avutil-59.dll', 'swresample-5.dll', 'ntpods-aac-encoder.exe', 'ntpods-eld-encoder.exe', 'ntpods-fdk-q31.dll', 'fix-driver.ps1', 'rename-mic.ps1', 'setup-common.ps1', 'audio-driver.ps1', 'restore-playback.ps1', 'stop-ntpods.ps1', 'msi-setup.ps1', 'uninstall.ps1', 'bluetooth-host-identity.ps1', 'winui', 'tools', 'licenses') {
            Copy-NTPodsAppEntry (Join-Path $root $name) (Join-Path $staged $name)
        }
        $oldFdk = Join-Path $staged 'fdk-aac.dll'
        if (Test-Path -LiteralPath $oldFdk) { Remove-Item -LiteralPath $oldFdk -Force -ErrorAction Stop }
        @(Get-InstallTree $staged) | Where-Object { -not $_.PSIsContainer } | Unblock-File
    } catch {
        $failure = $_
        @(Get-InstallTree $path) | Out-Null
        Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction Stop
        throw $failure
    }
}

function Install-NTPodsAppTransaction([string]$transactionId) {
    $saved = Read-NTPodsAppTransaction $transactionId
    if (-not $saved) { throw 'The app transaction is missing.' }
    $path = Get-NTPodsAppTransactionPath
    $dest = Join-Path $env:ProgramFiles 'NTPods'
    $staged = Join-Path $path 'new'
    $previous = Join-Path $path 'previous'
    if (-not (Test-Path -LiteralPath $staged -PathType Container) -or (Test-Path -LiteralPath $previous) -or
        (Test-Path -LiteralPath $dest) -ne $saved.HadInstall) { throw 'The app installation changed during setup.' }
    if ($saved.HadInstall) {
        Assert-ProtectedInstallPath $dest
        [IO.Directory]::Move($dest, $previous)
    }
    [IO.Directory]::Move($staged, $dest)
    Assert-ProtectedInstallPath $dest
}

function Restore-NTPodsAppTransaction([string]$transactionId) {
    $saved = Read-NTPodsAppTransaction $transactionId
    if (-not $saved) { return }
    $path = Get-NTPodsAppTransactionPath
    $dest = [IO.Path]::GetFullPath((Join-Path $env:ProgramFiles 'NTPods'))
    $previous = Join-Path $path 'previous'
    if ((Test-Path -LiteralPath $previous) -or (-not $saved.HadInstall -and -not (Test-Path -LiteralPath (Join-Path $path 'new')))) {
        if (Test-Path -LiteralPath $dest) {
            Assert-ProtectedInstallPath $dest
            Remove-Item -LiteralPath $dest -Recurse -Force -ErrorAction Stop
        }
        if ($saved.HadInstall) { [IO.Directory]::Move($previous, $dest) }
    }
    Complete-NTPodsAppTransaction $transactionId
}

function Complete-NTPodsAppTransaction([string]$transactionId) {
    if (-not (Read-NTPodsAppTransaction $transactionId)) { return }
    $path = Get-NTPodsAppTransactionPath
    Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction Stop
}
