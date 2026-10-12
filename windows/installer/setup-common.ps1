<#
    Shared install steps for install.ps1 (zip install) and msi-setup.ps1 (the MSI's
    custom actions). Dot-source it; it only defines functions.

    Everything per-user takes the user's SID and folders explicitly, because the
    MSI runs these steps as SYSTEM, where HKCU and $env:LOCALAPPDATA belong to
    SYSTEM and not to the person installing.
#>

$script:RunKeyPath = 'Software\Microsoft\Windows\CurrentVersion\Run'

function Get-CurrentUserSid {
    [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
}

function Assert-UserSid([string]$userSid) {
    if ($userSid -notmatch '^S-1-5-21-(\d+-){3}\d+$') { throw 'An explicit Windows account SID is required (not SYSTEM or a built-in group).' }
    $sid = New-Object Security.Principal.SecurityIdentifier($userSid)
    if ($sid.Value -cne $userSid) { throw 'The account SID must be canonical.' }
}

function New-DirectoryWithSecurity([string]$path, [Security.AccessControl.DirectorySecurity]$acl) {
    # Supplying the DACL during creation avoids inheriting ProgramData's user-write ACE.
    # Both APIs leave an existing directory unchanged; it must pass the checks below.
    if ($PSVersionTable.PSEdition -eq 'Desktop') {
        [IO.Directory]::CreateDirectory($path, $acl) | Out-Null
    } else {
        [IO.FileSystemAclExtensions]::CreateDirectory($acl, $path) | Out-Null
    }
}

function Initialize-SetupDirectory {
    $path = Join-Path $env:ProgramData 'NTPods'
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)')
    New-DirectoryWithSecurity $path $acl
    foreach ($entry in @(Get-InstallTree $path)) {
        if ($entry.PSIsContainer) {
            $acl = New-Object Security.AccessControl.DirectorySecurity
            $acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)')
        } else {
            $acl = New-Object Security.AccessControl.FileSecurity
            $acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)')
        }
        Set-Acl -LiteralPath $entry.FullName -AclObject $acl
    }
    $path
}

function Assert-TrustedInstallEntry([string]$path) {
    $trusted = @('S-1-5-18', 'S-1-5-32-544', 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
    $acl = Get-Acl -LiteralPath $path
    if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin $trusted) {
        throw "Setup refuses a user-owned path: $path"
    }
    $unsafeRights = [Security.AccessControl.FileSystemRights]'Write,Delete,DeleteSubdirectoriesAndFiles,ChangePermissions,TakeOwnership'
    foreach ($rule in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -eq 'Allow' -and $rule.IdentityReference.Value -notin $trusted -and
            -not ($rule.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -and
            ($rule.FileSystemRights -band $unsafeRights)) { throw "Setup refuses a user-writable path: $path" }
    }
}

# Never execute an elevated task from a directory the interactive user can edit.
function Get-InstallTree([string]$path) {
    $entry = Get-Item -LiteralPath $path -Force
    if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Install tree contains a reparse point: $path" }
    Assert-TrustedInstallEntry $path
    $entry
    if ($entry.PSIsContainer) {
        foreach ($child in Get-ChildItem -LiteralPath $path -Force) { Get-InstallTree $child.FullName }
    }
}

function Assert-ProtectedInstallPath([string]$path) {
    $full = [IO.Path]::GetFullPath($path).TrimEnd('\')
    $expected = [IO.Path]::GetFullPath((Join-Path $env:ProgramFiles 'NTPods')).TrimEnd('\')
    if ($full -ine $expected) { throw "NTPods must be installed in $expected." }
    $tree = @(Get-InstallTree $full)
    foreach ($entry in $tree) {
        if ((Get-Item -LiteralPath $entry.FullName -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Install tree changed during setup.' }
        if ($entry.PSIsContainer) {
            $acl = New-Object Security.AccessControl.DirectorySecurity
            $acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;BU)')
        } else {
            $acl = New-Object Security.AccessControl.FileSecurity
            $acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFX;;;BU)')
        }
        Set-Acl -LiteralPath $entry.FullName -AclObject $acl
    }
}

function Set-DriverUser([string]$service, [string]$userSid) {
    Assert-UserSid $userSid
    if ($service -notin 'NTPodsAAP', 'NTPodsMicACX') { throw 'Unexpected driver service.' }
    $key = "HKLM:\SYSTEM\CurrentControlSet\Services\$service\Parameters"
    New-Item -Path $key -Force | Out-Null
    $acl = New-Object Security.AccessControl.RegistrySecurity
    $acl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)')
    Set-Acl -LiteralPath $key -AclObject $acl
    New-ItemProperty -LiteralPath $key -Name AllowedUserSid -PropertyType String -Value $userSid -Force | Out-Null
}

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
}

# SystemStartOptions describes the current boot; bcdedit may describe changes
# that require a restart. The running boot must have test signing enabled.
function Test-TestMode {
    $opts = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control').SystemStartOptions
    $opts -match '\bTESTSIGNING\b'
}

# Secure Boot blocks `testsigning on`. Return $null when its state cannot be read
# (legacy BIOS or insufficient privileges).
function Test-SecureBoot {
    try { Confirm-SecureBootUEFI -ErrorAction Stop } catch { $null }
}

# BitLocker protection on the system drive. Changing Secure Boot or the boot
# options (testsigning) can make it ask for the recovery key at the next boot.
# WMI rather than Get-BitLockerVolume (missing on some editions) or manage-bde
# (its output is translated). Needs admin; $false when it can't be read.
function Test-BitLocker {
    try {
        $v = Get-CimInstance -Namespace 'root\CIMV2\Security\MicrosoftVolumeEncryption' `
            -ClassName Win32_EncryptableVolume -Filter "DriveLetter='$env:SystemDrive'" -ErrorAction Stop
        [bool]($v | Where-Object { $_.ProtectionStatus -eq 1 })
    } catch { $false }
}

$script:BitLockerAdvice = @'
Save your BitLocker recovery key before changing Secure Boot or Test Mode:
changing either can make Windows ask for it at the next boot, and without it
you lose access to this drive. Find it at https://aka.ms/myrecoverykey (if it
is saved to your Microsoft account) or print it from an admin terminal with:
  manage-bde -protectors -get C:
'@

# Only the hardened ACX device is supported by this build.
function Test-MicSupported {
    [Environment]::OSVersion.Version.Build -ge 22621
}

# Older Windows and the unhardened PortCls driver are outside this build.
function Resolve-MicDriver([string]$choice) {
    if ($choice -notin 'auto', 'acx' -or -not (Test-MicSupported)) {
        throw 'This build requires Windows 11 22H2 or newer and the hardened ACX microphone.'
    }
    'acx'
}

# Run a native tool, echo its output, and fail on an exit code outside $ok.
# (Native stderr must not become a terminating error under 'Stop', so the
# preference is relaxed for the call itself.)
function Invoke-Tool([string]$what, [int[]]$ok, [scriptblock]$cmd) {
    $ErrorActionPreference = 'Continue'
    $out = & $cmd 2>&1 | Out-String
    $code = $LASTEXITCODE
    if ($out.Trim()) { Write-Host $out.TrimEnd() }
    if ($ok -notcontains $code) { throw "$what failed (exit code $code)." }
}

. (Join-Path $PSScriptRoot 'stop-ntpods.ps1')

# Published names (oemNN.inf) of driver-store packages with this original INF
# name (and provider, when given). Uses DISM rather than parsing `pnputil
# /enum-drivers`, whose labels are translated on non-English Windows.
function Get-DriverPackages([string]$infName, [string[]]$provider) {
    @(Get-WindowsDriver -Online -ErrorAction Stop | Where-Object {
            (Split-Path -Leaf $_.OriginalFileName) -eq $infName -and
            (-not $provider -or $provider -contains $_.ProviderName)
        } | ForEach-Object { $_.Driver })
}

function Get-StartupDir([string]$appData) {
    Join-Path $appData 'Microsoft\Windows\Start Menu\Programs\Startup'
}

function Remove-RunValues([string]$userSid, [string[]]$names) {
    $key = "Registry::HKEY_USERS\$userSid\$script:RunKeyPath"
    foreach ($n in $names) { Remove-ItemProperty $key -Name $n -ErrorAction SilentlyContinue }
}

function Set-RunValues([string]$userSid, [string]$daemonExe, [string]$winuiExe) {
    $key = "Registry::HKEY_USERS\$userSid\$script:RunKeyPath"
    if (-not (Test-Path $key)) { New-Item $key -Force | Out-Null }
    Set-ItemProperty $key -Name 'NTPods Daemon' -Value "`"$daemonExe`""
    Set-ItemProperty $key -Name 'NTPods' -Value "`"$winuiExe`" --tray"
}

function Set-StartupMode([string]$userSid, [ValidateSet('on', 'off')][string]$mode, [string]$installDir) {
    Assert-UserSid $userSid
    $daemon = Join-Path $installDir 'ntpodsd.exe'
    $app = Join-Path $installDir 'winui\ntpods-winui.exe'
    if ($mode -eq 'on') { Set-RunValues $userSid $daemon $app }
    else { Remove-RunValues $userSid @('NTPods', 'NTPods Daemon') }
    $key = "Registry::HKEY_USERS\$userSid\$script:RunKeyPath"
    $values = if (Test-Path -LiteralPath $key -ErrorAction Stop) { Get-ItemProperty -LiteralPath $key -ErrorAction Stop }
    if ($mode -eq 'on') {
        if ($values.NTPods -ine "`"$app`" --tray" -or $values.'NTPods Daemon' -ine "`"$daemon`"") {
            throw 'Windows did not save both NTPods startup entries.'
        }
    } elseif ($null -ne $values.NTPods -or $null -ne $values.'NTPods Daemon') {
        throw 'Windows did not remove both NTPods startup entries.'
    }
}

# Remove every copy of a test certificate from the machine stores. Uses the
# X509Store API (the same one that adds them): piping the Cert: provider into
# Remove-Item left all of them in place when run as SYSTEM from the MSI.
function Remove-TestCert([string]$subject) {
    foreach ($name in 'My', 'Root', 'TrustedPublisher') {
        $s = New-Object System.Security.Cryptography.X509Certificates.X509Store($name, 'LocalMachine')
        try {
            $s.Open('ReadWrite')
            $old = @($s.Certificates | Where-Object { $_.Subject -eq $subject })
            foreach ($c in $old) { $s.Remove($c) }
            if ($old) { Write-Host "==> Removed $($old.Count) '$subject' from $name" }
        } catch {
            throw "Could not remove '$subject' from ${name}: $_"
        } finally { $s.Close() }
    }
}

# Only clean our protected work directory, never user-controlled profile paths.
function Remove-SetupLeftovers([string]$localAppData) {
    $base = [IO.Path]::GetFullPath((Join-Path $env:ProgramData 'NTPods'))
    Get-ChildItem -LiteralPath $base -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^drivers-[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$' } |
        ForEach-Object {
            $target = [IO.Path]::GetFullPath($_.FullName)
            if (-not $target.StartsWith($base + '\', [StringComparison]::OrdinalIgnoreCase) -or
                ($_.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Unsafe setup cleanup path.' }
            @(Get-InstallTree $target) | Out-Null
            Remove-Item -LiteralPath $target -Recurse -Force
        }
}

# ---- drivers ------------------------------------------------------------------
function Get-DriverCheckpointPath {
    Join-Path $env:ProgramData 'NTPods\driver-rollback'
}

function Get-NTPodsDriverPackages {
    @(Get-WindowsDriver -Online -ErrorAction Stop | Where-Object {
        $_.ProviderName -eq 'NTPods' -and
        (Split-Path -Leaf $_.OriginalFileName) -in 'NTPodsAAP.inf', 'AudioCodec.inf', 'NTPodsMicPC.inf'
    })
}

function Get-NTPodsCertificates {
    foreach ($name in 'My', 'Root', 'TrustedPublisher') {
        foreach ($cert in Get-ChildItem "Cert:\LocalMachine\$name" | Where-Object Subject -eq 'CN=NTPods Test Cert') {
            [pscustomobject]@{ Store = $name; Thumbprint = $cert.Thumbprint }
        }
    }
}

function Remove-NTPodsDriverCheckpointFiles {
    $path = [IO.Path]::GetFullPath((Get-DriverCheckpointPath))
    $expected = [IO.Path]::GetFullPath((Join-Path $env:ProgramData 'NTPods\driver-rollback'))
    if ($path -ine $expected) { throw 'Unsafe driver backup cleanup path.' }
    if (Test-Path -LiteralPath $path) {
        @(Get-InstallTree $path) | Out-Null
        Remove-Item -LiteralPath $path -Recurse -Force
    }
}

function Read-NTPodsDriverCheckpoint([string]$transactionId) {
    if ([string]::IsNullOrEmpty($transactionId)) { throw 'A driver transaction ID is required.' }
    $path = Get-DriverCheckpointPath
    if (-not (Test-Path -LiteralPath $path)) { return }
    @(Get-InstallTree $path) | Out-Null
    $saved = Get-Content -LiteralPath (Join-Path $path 'checkpoint.json') -Raw | ConvertFrom-Json
    if (-not [string]::Equals($saved.TransactionId, $transactionId, [StringComparison]::Ordinal)) {
        throw "The driver backup belongs to another transaction and has been retained: $path"
    }
    $saved
}

function Complete-NTPodsDriverTransaction([string]$transactionId) {
    if (Read-NTPodsDriverCheckpoint $transactionId) { Remove-NTPodsDriverCheckpointFiles }
}

function Save-NTPodsDriverCheckpoint([string]$transactionId) {
    if ([string]::IsNullOrEmpty($transactionId)) { throw 'A driver transaction ID is required.' }
    $path = Get-DriverCheckpointPath
    if (Test-Path -LiteralPath $path) { throw "An earlier driver backup still needs recovery: $path" }
    New-Item -ItemType Directory -Path $path -ErrorAction Stop | Out-Null
    try {
        $packages = @(foreach ($driver in Get-NTPodsDriverPackages) {
            if ($driver.Driver -notmatch '^oem\d+\.inf$') { throw 'Unexpected published driver name.' }
            $folder = Join-Path $path $driver.Driver
            New-Item -ItemType Directory -Path $folder | Out-Null
            Invoke-Tool 'Backing up the installed driver' @(0) { pnputil /export-driver $driver.Driver $folder }
            $name = Split-Path -Leaf $driver.OriginalFileName
            $inf = @(Get-ChildItem -LiteralPath $folder -Filter $name -File -Recurse)
            if ($inf.Count -ne 1) { throw "Driver backup did not contain exactly one $name." }
            [pscustomobject]@{ Published = $driver.Driver; Name = $name; Inf = $inf[0].FullName.Substring($path.Length + 1) }
        })
        $devices = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | Where-Object {
            $_.HardwareID -contains 'ROOT\NTPodsMicACX' -or $_.HardwareID -contains 'ROOT\NTPodsMicPC'
        } | ForEach-Object {
            $hwid = if ($_.HardwareID -contains 'ROOT\NTPodsMicACX') { 'ROOT\NTPodsMicACX' } else { 'ROOT\NTPodsMicPC' }
            $inf = (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName 'DEVPKEY_Device_DriverInfPath' -ErrorAction Stop).Data
            if ($inf -notin $packages.Published) { throw "Cannot back up the driver for $hwid." }
            [pscustomobject]@{ HardwareId = $hwid; Published = $inf }
        })
        $policy = @(foreach ($service in 'NTPodsAAP', 'NTPodsMicACX') {
            $sid = (Get-ItemProperty -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Services\$service\Parameters" -ErrorAction SilentlyContinue).AllowedUserSid
            if ($sid) { Assert-UserSid $sid }
            [pscustomobject]@{ Service = $service; Sid = $sid }
        })
        [pscustomobject]@{ TransactionId = $transactionId; Packages = $packages; Devices = $devices; Policy = $policy; Certificates = @(Get-NTPodsCertificates) } |
            ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $path 'checkpoint.json') -Encoding UTF8
    } catch {
        # No driver or certificate has changed before this checkpoint is complete.
        Remove-NTPodsDriverCheckpointFiles
        throw
    }
}

function Restore-NTPodsDriverCheckpoint([string]$root, [string]$transactionId) {
    $path = Get-DriverCheckpointPath
    $saved = Read-NTPodsDriverCheckpoint $transactionId
    if (-not $saved) { return }
    $mutated = Test-Path -LiteralPath (Join-Path $path 'replacing')
    $devcon = Join-Path $root 'tools\devcon.exe'
    if ($mutated) { Remove-MicDevices $devcon }
    foreach ($driver in Get-NTPodsDriverPackages) {
        if ($mutated -or $driver.Driver -notin $saved.Packages.Published) {
            Invoke-Tool 'Removing the failed driver replacement' @(0, 3010) { pnputil /delete-driver $driver.Driver /uninstall /force }
        }
    }
    if ($mutated) {
        foreach ($item in $saved.Policy) {
            if ($item.Sid) { Set-DriverUser $item.Service $item.Sid }
            else { Remove-ItemProperty -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Services\$($item.Service)\Parameters" -Name AllowedUserSid -ErrorAction SilentlyContinue }
        }
        foreach ($package in $saved.Packages) {
            $inf = [IO.Path]::GetFullPath((Join-Path $path $package.Inf))
            if (-not $inf.StartsWith($path + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid driver backup path.' }
            if ($package.Name -ieq 'NTPodsAAP.inf') {
                Invoke-Tool 'Restoring the Bluetooth driver' @(0, 259, 3010) { pnputil /add-driver $inf /install }
            } else {
                Invoke-Tool 'Restoring the microphone package' @(0, 259, 3010) { pnputil /add-driver $inf }
            }
        }
        foreach ($device in $saved.Devices) {
            $package = @($saved.Packages | Where-Object Published -eq $device.Published)
            if ($package.Count -ne 1 -or $device.HardwareId -notin 'ROOT\NTPodsMicACX', 'ROOT\NTPodsMicPC') { throw 'Invalid microphone backup.' }
            $inf = Join-Path $path $package[0].Inf
            Invoke-Tool 'Restoring the microphone device' @(0, 1) { & $devcon install $inf $device.HardwareId }
        }
    }
    foreach ($cert in Get-NTPodsCertificates) {
        if (-not @($saved.Certificates | Where-Object { $_.Store -eq $cert.Store -and $_.Thumbprint -eq $cert.Thumbprint }).Count) {
            Remove-NTPodsCertificate $cert.Store $cert.Thumbprint
        }
    }
    Complete-NTPodsDriverTransaction $transactionId
}

function Remove-NTPodsCertificate([string]$storeName, [string]$thumbprint) {
    $store = New-Object System.Security.Cryptography.X509Certificates.X509Store($storeName, 'LocalMachine')
    try {
        $store.Open('ReadWrite')
        foreach ($item in @($store.Certificates | Where-Object Thumbprint -eq $thumbprint)) { $store.Remove($item) }
    } finally { $store.Close() }
}

function Get-NTPodsSigningCertificate {
    $cert = Get-ChildItem Cert:\LocalMachine\My |
        Where-Object { $_.Subject -eq 'CN=NTPods Test Cert' -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date).AddDays(30) } |
        Sort-Object NotAfter -Descending | Select-Object -First 1
    if ($cert) {
        Write-Host '==> Reusing the NTPods test code-signing certificate...'
    } else {
        Write-Host '==> Creating a test code-signing certificate...'
        $cert = New-SelfSignedCertificate -Type CodeSigningCert `
            -Subject 'CN=NTPods Test Cert' `
            -CertStoreLocation Cert:\LocalMachine\My `
            -KeyUsage DigitalSignature -KeyExportPolicy Exportable
    }
    foreach ($name in 'Root', 'TrustedPublisher') {
        $store = New-Object System.Security.Cryptography.X509Certificates.X509Store($name, 'LocalMachine')
        try { $store.Open('ReadWrite'); $store.Add($cert) }
        finally { $store.Close() }
    }
    $cert
}

function Set-NTPodsPackageSignature([string]$path, $cert) {
    $result = Set-AuthenticodeSignature -FilePath $path -Certificate $cert -HashAlgorithm SHA256
    if (-not $result.SignerCertificate) { throw "Could not sign ${path}: $($result.StatusMessage)" }
    if ($result.Status -ne 'Valid') { Write-Warning "$(Split-Path -Leaf $path): signed, but reported $($result.Status): $($result.StatusMessage)" }
}

function Assert-AapBinding {
    $devices = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | Where-Object {
        $_.InstanceId -like 'BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}*'
    })
    if (-not $devices.Count) {
        Write-Host '==> NTPodsAAP package staged; no present AirPods AAP service device is available to bind yet.'
        return
    }
    foreach ($device in $devices) {
        $service = (Get-PnpDeviceProperty -InstanceId $device.InstanceId -KeyName 'DEVPKEY_Device_Service' -ErrorAction Stop).Data
        if ($service -ne 'NTPodsAAP') {
            throw "NTPodsAAP was staged but did not bind the present AirPods service device: $($device.InstanceId). Setup cannot report a working Bluetooth driver."
        }
    }
    Write-Host "==> NTPodsAAP is bound to $($devices.Count) present AirPods service device(s)."
}

function Remove-MicDevices([string]$devcon) {
    # DevCon returns 0 for success, 1 for a required reboot, and 2 for failure.
    Invoke-Tool 'Removing the old mic device (devcon)' @(0, 1) { & $devcon remove 'ROOT\NTPodsMicPC' }
    Invoke-Tool 'Removing the old ACX mic device (devcon)' @(0, 1) { & $devcon remove 'ROOT\NTPodsMicACX' }
}

function Assert-MicDevice([string]$hardwareId) {
    $devices = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | Where-Object { $_.HardwareID -contains $hardwareId })
    if (-not $devices.Count) { throw "devcon reported success, but no $hardwareId device exists." }
    foreach ($device in $devices) {
        $problem = (Get-PnpDeviceProperty -InstanceId $device.InstanceId -KeyName 'DEVPKEY_Device_ProblemCode' -ErrorAction Stop).Data
        if ($null -eq $problem) { throw "Windows did not report the microphone device status: $($device.InstanceId)." }
        if ($problem -eq 14) {
            # CM_PROB_NEED_RESTART is pending completion, not a failed driver start.
            Write-Host "==> The microphone device requires a Windows restart: $($device.InstanceId)."
        } elseif ($problem -ne 0) {
            throw "The microphone driver did not start (Device Manager code $problem): $($device.InstanceId)."
        }
    }
}

# $root holds driver\, driver-mic-acx\ and tools\devcon.exe
# (the dist / install dir). The packages are copied to $work and signed there, so
# the files the MSI installed stay byte-identical (a repair would otherwise see
# them as changed). $micDriver: auto | acx, see Resolve-MicDriver.
function Install-NTPodsDrivers([string]$root, [string]$work, [string]$micDriver = 'auto', [string]$userSid, [string]$transactionId) {
    Assert-UserSid $userSid
    if ((Resolve-MicDriver $micDriver) -ne 'acx') { throw 'This experimental build supports only the hardened ACX microphone on Windows 11 22H2 or newer.' }
    $devcon = Join-Path $root 'tools\devcon.exe'
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    foreach ($d in 'driver', 'driver-mic-acx') {
        Copy-Item (Join-Path $root $d) $work -Recurse -Force
    }
    $aap = @{ sys = Join-Path $work 'driver\NTPodsAAP.sys'; cat = Join-Path $work 'driver\ntpodsaap.cat'; inf = Join-Path $work 'driver\NTPodsAAP.inf' }
    $mic = @{ name = 'ACX'; hwid = 'ROOT\NTPodsMicACX'
        sys = Join-Path $work 'driver-mic-acx\AudioCodec.sys'; cat = Join-Path $work 'driver-mic-acx\audiocodec.cat'; inf = Join-Path $work 'driver-mic-acx\AudioCodec.inf' }

    Save-NTPodsDriverCheckpoint $transactionId
    try {

        $cert = Get-NTPodsSigningCertificate
        # Embedded signatures preserve the Authenticode hashes covered by the catalogs.
        Set-NTPodsPackageSignature $aap.sys $cert
        Set-NTPodsPackageSignature $aap.cat $cert
        Set-NTPodsPackageSignature $mic.sys $cert
        Set-NTPodsPackageSignature $mic.cat $cert
        # Validate both signed packages before unbinding either installed driver.
        Invoke-Tool 'Staging NTPodsAAP' @(0, 259, 3010) { pnputil /add-driver $aap.inf }
        Invoke-Tool 'Staging NTPodsMic' @(0, 259, 3010) { pnputil /add-driver $mic.inf }
        $checkpoint = Get-DriverCheckpointPath
        $saved = Get-Content -LiteralPath (Join-Path $checkpoint 'checkpoint.json') -Raw | ConvertFrom-Json
        Set-Content -LiteralPath (Join-Path $checkpoint 'replacing') -Value 'Driver replacement started.'

        # NTPodsAAP: PnP profile driver, via pnputil.
        Write-Host '==> Removing any previously installed NTPodsAAP package...'
        foreach ($oem in @($saved.Packages | Where-Object Name -eq 'NTPodsAAP.inf' | ForEach-Object Published)) {
            Invoke-Tool 'Removing the old NTPodsAAP package' @(0, 3010) { pnputil /delete-driver $oem /uninstall /force }
        }
        Set-DriverUser 'NTPodsAAP' $userSid
        Write-Host '==> Installing NTPodsAAP...'
        # 259 = added, but no matching device yet (AirPods not paired); 3010 = reboot needed.
        Invoke-Tool 'Installing NTPodsAAP (pnputil)' @(0, 259, 3010) { pnputil /add-driver $aap.inf /install }
        Assert-AapBinding

        # NTPodsMic: ROOT-enumerated device, via devcon. Remove both kinds first, so
        # switching between ACX and PortCls never leaves two mics side by side.
        Write-Host '==> Removing any existing mic device...'
        Remove-MicDevices $devcon
        # Old mic packages pile up in the driver store otherwise (one per install).
        foreach ($oem in @($saved.Packages | Where-Object Name -ne 'NTPodsAAP.inf' | ForEach-Object Published)) {
            Invoke-Tool 'Removing the old microphone package' @(0, 3010) { pnputil /delete-driver $oem /uninstall /force }
        }
        Start-Sleep -Seconds 1
        Write-Host "==> Installing NTPodsMic, $($mic.name) driver (virtual microphone)..."
        Set-DriverUser 'NTPodsMicACX' $userSid
        # devcon: 0 = done, 1 = done but a reboot is needed.
        Invoke-Tool 'Installing NTPodsMic (devcon)' @(0, 1) { & $devcon install $mic.inf $mic.hwid }
        Assert-MicDevice $mic.hwid
    } catch {
        $failure = $_
        try { Restore-NTPodsDriverCheckpoint $root $transactionId }
        catch { throw "Driver installation failed: $failure Recovery also failed: $_ The backup remains at $(Get-DriverCheckpointPath)." }
        throw $failure
    }
}

function Uninstall-NTPodsDrivers([string]$root) {
    $devcon = Join-Path $root 'tools\devcon.exe'
    if (Test-Path $devcon) {
        Write-Host '==> Removing the NTPods microphone device...'
        Remove-MicDevices $devcon
    }
    foreach ($oem in (Get-DriverPackages 'NTPodsAAP.inf' @('NTPods'))) {
        Write-Host "==> Removing driver package $oem (NTPodsAAP.inf)"
        Invoke-Tool 'Removing NTPodsAAP' @(0, 3010) { pnputil /delete-driver $oem /uninstall /force }
    }
    Remove-MicDriverPackages
    Remove-TestCert 'CN=NTPods Test Cert'
    foreach ($service in 'NTPodsAAP', 'NTPodsMicACX') {
        Remove-ItemProperty -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Services\$service\Parameters" -Name AllowedUserSid -ErrorAction SilentlyContinue
    }
}

# Remove every mic driver package from the driver store, PortCls and ACX alike.
# The ACX INF is named after the WDK sample it came from, so
# match the NTPods provider too, never generic sample or audio-sink packages.
function Remove-MicDriverPackages {
    foreach ($p in @(@('NTPodsMicPC.inf', @('NTPods')), @('audiocodec.inf', @('NTPods')))) {
        foreach ($oem in (Get-DriverPackages $p[0] $p[1])) {
            Write-Host "==> Removing driver package $oem ($($p[0]))"
            Invoke-Tool "Removing $($p[0])" @(0, 3010) { pnputil /delete-driver $oem /uninstall /force }
        }
    }
}

# ---- elevated on-demand helper tasks -----------------------------------------
# The unelevated daemon invokes these pre-authorized elevated tasks:
# - Fix Driver checks and recovers its AAP devnode after repeated open failures.
# - Rename Mic reads micname.txt and updates the virtual microphone's name.
# Both run as the installing user, whose LOCALAPPDATA holds micname.txt.
function Register-NTPodsTasks([string]$scriptDir, [string]$userSid) {
    Assert-UserSid $userSid
    Assert-ProtectedInstallPath $scriptDir
    $user = (New-Object Security.Principal.SecurityIdentifier($userSid)).Translate([Security.Principal.NTAccount]).Value
    Write-Host "==> Registering the elevated helper tasks for $user..."
    foreach ($t in @(
            @('NTPods Fix Driver', 'fix-driver.ps1', 'Recover the NTPods AAP devnode from Code 38 (no reboot).'),
            @('NTPods Rename Mic', 'rename-mic.ps1', 'Rename the NTPods virtual mic to the connected device name.'))) {
        # PowerShell's WindowStyle hides its console only after allocation.
        $action = New-ScheduledTaskAction -Execute (Join-Path $scriptDir 'ntpodsd.exe') `
            -Argument "--run-helper $([IO.Path]::GetFileNameWithoutExtension($t[1]))"
        $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries `
            -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 2) -StartWhenAvailable
        Register-ScheduledTask -TaskName $t[0] -Action $action -Principal $principal `
            -Settings $settings -Description $t[2] -Force | Out-Null
    }
}

function Unregister-NTPodsTasks {
    foreach ($t in 'NTPods Fix Driver', 'NTPods Rename Mic') {
        Unregister-ScheduledTask -TaskName $t -Confirm:$false -ErrorAction SilentlyContinue
    }
}
