# No elevation or system changes: temporary filesystem checks, then mocked privileged writes.
$ErrorActionPreference = 'Stop'
& (Join-Path $PSScriptRoot 'test-stop-ntpods.ps1')
. (Join-Path $PSScriptRoot 'setup-common.ps1')
function Assert-Fails([scriptblock]$body) {
    $failed = $false
    try { & $body } catch { $failed = $true }
    if (-not $failed) { throw 'Expected rejection.' }
}
# Real NTFS regression: a writable parent must not leak its ACL into a fresh directory.
$testParent = [IO.Path]::GetFullPath((Join-Path $env:TEMP ('ntpods-setup-test-' + [guid]::NewGuid())))
$tempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')
if (-not $testParent.StartsWith($tempRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe test directory.' }
$testUser = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$parentAcl = New-Object Security.AccessControl.DirectorySecurity
$parentAcl.SetSecurityDescriptorSddlForm("O:${testUser}D:P(A;OICI;FA;;;${testUser})(A;OICI;FA;;;BU)")
$privateAcl = New-Object Security.AccessControl.DirectorySecurity
$privateAcl.SetSecurityDescriptorSddlForm("O:${testUser}D:P(A;OICI;FA;;;${testUser})")
try {
    New-DirectoryWithSecurity $testParent $parentAcl
    $fresh = Join-Path $testParent 'fresh'
    New-DirectoryWithSecurity $fresh $privateAcl
    $createdAcl = Get-Acl -LiteralPath $fresh
    if (-not $createdAcl.AreAccessRulesProtected -or $createdAcl.Sddl -match ';;;BU') { throw 'Fresh setup directory inherited user-write permissions.' }
    $existing = Join-Path $testParent 'existing'
    [IO.Directory]::CreateDirectory($existing) | Out-Null
    $before = (Get-Acl -LiteralPath $existing).Sddl
    New-DirectoryWithSecurity $existing $privateAcl
    if ((Get-Acl -LiteralPath $existing).Sddl -ne $before) { throw 'Existing directory ACL was changed before validation.' }
} finally {
    if (Test-Path -LiteralPath $testParent) { Remove-Item -LiteralPath $testParent -Recurse -Force }
}
$sid = 'S-1-5-21-123-456-789-1001'
Assert-UserSid $sid
foreach ($bad in @('', 'S-1-5-18', 'S-1-5-32-544', 'S-1-12-1-1-2-3-4', "$sid;whoami", 'S-1-5-21-1-2-3-4294967296')) {
    Assert-Fails { Assert-UserSid $bad }
}
# Startup updates use the installed user's hive and must verify both writes/removals.
& {
    $script:runValues = @{}
    $script:ignoreStartupWrite = $false
    $script:ignoreStartupRemove = $false
    $expectedKey = "Registry::HKEY_USERS\$sid\$script:RunKeyPath"
    function Test-Path { param($Path, $LiteralPath, $ErrorAction)
        if (($Path -and $Path -ne $expectedKey) -or ($LiteralPath -and $LiteralPath -ne $expectedKey)) { throw 'Wrong startup hive.' }
        $true
    }
    function Get-ItemProperty { param($LiteralPath, $ErrorAction)
        if ($LiteralPath -ne $expectedKey) { throw 'Wrong startup read.' }
        [pscustomobject]$script:runValues
    }
    function Set-ItemProperty { param($Path, $Name, $Value)
        if ($Path -ne $expectedKey) { throw 'Wrong startup write.' }
        if (-not ($script:ignoreStartupWrite -and $Name -eq 'NTPods')) { $script:runValues[$Name] = $Value }
    }
    function Remove-ItemProperty { param($Path, $Name, $ErrorAction)
        if ($Path -ne $expectedKey) { throw 'Wrong startup removal.' }
        if (-not $script:ignoreStartupRemove) { $script:runValues.Remove($Name) }
    }
    $directory = 'C:\Program Files\NTPods'
    Set-StartupMode $sid 'on' $directory
    if ($script:runValues['NTPods'] -cne '"C:\Program Files\NTPods\winui\ntpods-winui.exe" --tray' -or
        $script:runValues['NTPods Daemon'] -cne '"C:\Program Files\NTPods\ntpodsd.exe"') { throw 'Startup command quoting changed.' }
    $script:ignoreStartupRemove = $true
    Assert-Fails { Set-StartupMode $sid 'off' $directory }
    $script:ignoreStartupRemove = $false
    Set-StartupMode $sid 'off' $directory
    if ($script:runValues.Count) { throw 'Disabled startup left an entry.' }
    $script:ignoreStartupWrite = $true
    Assert-Fails { Set-StartupMode $sid 'on' $directory }
    Assert-Fails { Set-StartupMode 'S-1-5-18' 'on' $directory }
}
# Tasks enter the windowless executable; PowerShell is never their direct action.
& {
    $script:taskActions = @()
    $directory = Join-Path $env:ProgramFiles 'NTPods'
    function Assert-ProtectedInstallPath { param($path)
        if ($path -ne $directory) { throw 'Task helper path escaped the protected installation.' }
    }
    function New-ScheduledTaskAction { param($Execute, $Argument)
        [pscustomobject]@{ Execute = $Execute; Arguments = $Argument }
    }
    function New-ScheduledTaskPrincipal { param($UserId, $LogonType, $RunLevel)
        if (-not $UserId -or $LogonType -ne 'Interactive' -or $RunLevel -ne 'Highest') { throw 'Helper task identity changed.' }
        'principal'
    }
    function New-ScheduledTaskSettingsSet { param([switch]$AllowStartIfOnBatteries, [switch]$DontStopIfGoingOnBatteries, $ExecutionTimeLimit, [switch]$StartWhenAvailable)
        if ($ExecutionTimeLimit.TotalMinutes -ne 2) { throw 'Helper task limit changed.' }
        'settings'
    }
    function Register-ScheduledTask { param($TaskName, $Action, $Principal, $Settings, $Description, [switch]$Force)
        $script:taskActions += [pscustomobject]@{ Name = $TaskName; Action = $Action }
    }
    Register-NTPodsTasks $directory $testUser
    if ($script:taskActions.Count -ne 2) { throw 'Expected both helper tasks.' }
    foreach ($task in $script:taskActions) {
        $expected = if ($task.Name -eq 'NTPods Fix Driver') { '--run-helper fix-driver' } else { '--run-helper rename-mic' }
        if ($task.Action.Execute -cne (Join-Path $directory 'ntpodsd.exe') -or $task.Action.Arguments -cne $expected) {
            throw 'Helper task must use the fixed console-free launch command.'
        }
    }
}
$script:writes = @()
function New-Item { param($Path, [switch]$Force) $script:writes += "create:$Path" }
function Set-Acl { param($LiteralPath, $AclObject) $script:writes += "acl:$LiteralPath"; $script:descriptor = $AclObject.GetSecurityDescriptorSddlForm('All') }
function New-ItemProperty { param($LiteralPath, $Name, $PropertyType, $Value, [switch]$Force) $script:writes += "set:${LiteralPath}:${Name}:${Value}" }
Set-DriverUser 'NTPodsAAP' $sid
if ($script:writes.Count -ne 3 -or $script:writes[2] -cne "set:HKLM:\SYSTEM\CurrentControlSet\Services\NTPodsAAP\Parameters:AllowedUserSid:$sid") { throw 'Wrong driver identity write.' }
if ($script:descriptor -notmatch 'D:P' -or $script:descriptor -match ';;;BU|;;;WD|;;;AU') { throw 'Driver policy registry key grants general user access.' }
$script:writes = @()
Set-DriverUser 'NTPodsMicACX' $sid
if ($script:writes[2] -cne "set:HKLM:\SYSTEM\CurrentControlSet\Services\NTPodsMicACX\Parameters:AllowedUserSid:$sid") { throw 'Wrong ACX identity write.' }
$script:writes = @()
Assert-Fails { Set-DriverUser 'OtherDriver' $sid }
Assert-Fails { Set-DriverUser 'AudioCodec' $sid }
Assert-Fails { Set-DriverUser 'NTPodsAAP' 'S-1-5-18' }
if ($script:writes.Count) { throw 'Rejected identity changed the system.' }
function Get-WindowsDriver { param([switch]$Online, $ErrorAction)
    if ($script:driverQueryFailure) {
        Write-Error 'Simulated driver-store query failure.' -ErrorAction $ErrorAction
        return
    }
    @(
        [pscustomobject]@{ OriginalFileName = 'C:\DriverStore\NTPodsAAP.inf'; ProviderName = 'NTPods'; Driver = 'oem10.inf' },
        [pscustomobject]@{ OriginalFileName = 'C:\DriverStore\AudioCodec.inf'; ProviderName = 'NTPods'; Driver = 'oem11.inf' },
        [pscustomobject]@{ OriginalFileName = 'C:\DriverStore\AudioCodec.inf'; ProviderName = 'Luculent'; Driver = 'oem21.inf' }
    )
}
if ((@(Get-DriverPackages 'NTPodsAAP.inf' @('NTPods')) -join ',') -cne 'oem10.inf') { throw 'AAP package selection touches another provider.' }
if ((@(Get-DriverPackages 'AudioCodec.inf' @('NTPods')) -join ',') -cne 'oem11.inf') { throw 'Mic package selection touches Alternative A2DP.' }
$script:driverQueryFailure = $true
Assert-Fails { Get-DriverPackages 'NTPodsAAP.inf' @('NTPods') }
$script:driverQueryFailure = $false
function Invoke-MockDevcon { param($command, $hardwareId)
    if ($command -ne 'remove' -or $hardwareId -notin 'ROOT\NTPodsMicPC', 'ROOT\NTPodsMicACX') { throw 'Unexpected device removal.' }
    $script:removalCalls++
    $global:LASTEXITCODE = $script:removalExit
}
foreach ($code in 0, 1) {
    $script:removalExit = $code
    $script:removalCalls = 0
    Remove-MicDevices 'Invoke-MockDevcon'
    if ($script:removalCalls -ne 2) { throw 'Successful removal did not check both microphone devices.' }
}
$script:removalExit = 2
$script:removalCalls = 0
Assert-Fails { Remove-MicDevices 'Invoke-MockDevcon' }
if ($script:removalCalls -ne 1) { throw 'Removal continued after DevCon failed.' }
$global:LASTEXITCODE = 0
$script:certificateRemovalCalls = 0
$script:certificateCloseCalls = 0
$script:certificateStore = [pscustomobject]@{ Certificates = @([pscustomobject]@{ Subject = 'CN=NTPods Test Cert' }) }
$script:certificateStore | Add-Member ScriptMethod Open { param($mode) }
$script:certificateStore | Add-Member ScriptMethod Remove { param($certificate)
    $script:certificateRemovalCalls++
    throw 'Simulated certificate removal failure.'
}
$script:certificateStore | Add-Member ScriptMethod Close { $script:certificateCloseCalls++ }
function New-Object { param([string]$TypeName, [object[]]$ArgumentList)
    if ($TypeName -ne 'System.Security.Cryptography.X509Certificates.X509Store') { throw 'Unexpected object creation in certificate test.' }
    $script:certificateStore
}
try {
    Assert-Fails { Remove-TestCert 'CN=NTPods Test Cert' }
    if ($script:certificateRemovalCalls -ne 1 -or $script:certificateCloseCalls -ne 1) { throw 'Certificate failure did not stop cleanup and close the mocked store.' }
} finally {
    Remove-Item Function:\New-Object
}
$aapInf = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\drivers\aap\NTPodsAAP.inf') -Raw
if ($aapInf -match '0000110b|UpperFilters|LowerFilters' -or $aapInf -notmatch '74ec2172-0bad-4d01-8f77-997b2be0722a') { throw 'AAP INF must bind only the Apple service, without audio-stack filters.' }
$script:deviceIds = @('BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}\AudioSink')
$script:deviceService = $null
$script:deviceQueries = 0
function Get-PnpDevice { param([switch]$PresentOnly, $ErrorAction)
    if (-not $PresentOnly) { throw 'Binding check must exclude absent service nodes.' }
    $script:deviceIds | ForEach-Object { [pscustomobject]@{ InstanceId = $_ } }
}
function Get-PnpDeviceProperty { param($InstanceId, $KeyName, $ErrorAction)
    if ($KeyName -ne 'DEVPKEY_Device_Service' -or $InstanceId -notlike 'BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}*') { throw 'Binding check touched an unrelated device.' }
    $script:deviceQueries++
    [pscustomobject]@{ Data = $script:deviceService }
}
Assert-AapBinding # No AirPods yet: staging is allowed, AudioSink is untouched.
if ($script:deviceQueries) { throw 'Binding check queried AudioSink.' }
$script:deviceIds += 'BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}\AirPods'
Assert-Fails { Assert-AapBinding }
$script:deviceService = 'WrongDriver'
Assert-Fails { Assert-AapBinding }
$script:deviceService = 'NTPodsAAP'
Assert-AapBinding
$install = Join-Path $env:ProgramFiles 'NTPods'
$child = Join-Path $install 'helper.ps1'
$script:childAttributes = [IO.FileAttributes]::ReparsePoint
$script:existingAcl = New-Object Security.AccessControl.DirectorySecurity
$script:existingAcl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;BU)')
function Get-Acl { param($LiteralPath) $script:existingAcl }
function Get-Item { param($LiteralPath, [switch]$Force)
    [pscustomobject]@{ FullName = $LiteralPath; PSIsContainer = ($LiteralPath -eq $install); Attributes = $(if ($LiteralPath -eq $install) { [IO.FileAttributes]::Directory } else { $script:childAttributes }) }
}
function Get-ChildItem { param($LiteralPath, [switch]$Force) Get-Item -LiteralPath $child }
$script:writes = @()
Assert-Fails { Assert-ProtectedInstallPath (Join-Path $env:TEMP 'NTPods') }
Assert-Fails { Assert-ProtectedInstallPath $install }
if ($script:writes.Count) { throw 'A rejected install path changed ACLs.' }
$script:childAttributes = [IO.FileAttributes]::Normal
Assert-ProtectedInstallPath $install
if ($script:writes.Count -ne 2 -or $script:writes[1] -ne "acl:$child") { throw 'Protected children were not secured individually.' }
if ($script:descriptor -notmatch 'O:BA' -or $script:descriptor -notmatch 'D:P' -or $script:descriptor -match 'FW;;;BU|FA;;;BU') { throw 'Helper remains user-writable or user-owned.' }
$script:writes = @()
$script:existingAcl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;BU)')
Assert-Fails { Assert-ProtectedInstallPath $install }
$script:existingAcl.SetSecurityDescriptorSddlForm("O:${sid}G:BAD:P(A;OICI;FA;;;BA)")
Assert-Fails { Assert-ProtectedInstallPath $install }
if ($script:writes.Count) { throw 'An untrusted existing tree was modified instead of rejected.' }
# Fresh initialization receives its protected ACL before the first trust check.
$script:created = $false
function New-DirectoryWithSecurity { param($path, $acl)
    if ($path -ne (Join-Path $env:ProgramData 'NTPods') -or -not $acl.AreAccessRulesProtected) { throw 'Wrong setup directory creation policy.' }
    $script:existingAcl = $acl
    $script:created = $true
}
function Get-Acl { param($LiteralPath)
    if (-not $script:created) { throw 'Fresh directory inspected before protected creation.' }
    $script:existingAcl
}
function Get-Item { param($LiteralPath, [switch]$Force)
    [pscustomobject]@{ FullName = $LiteralPath; PSIsContainer = $true; Attributes = [IO.FileAttributes]::Directory }
}
function Get-ChildItem { param($LiteralPath, [switch]$Force) }
$script:writes = @()
$initialized = Initialize-SetupDirectory
if ($initialized -ne (Join-Path $env:ProgramData 'NTPods') -or $script:writes.Count -ne 1) { throw 'Fresh setup directory initialization failed.' }
$script:writes = @()
$script:existingAcl.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;BU)')
function New-DirectoryWithSecurity { param($path, $acl) } # Existing paths are returned unchanged.
Assert-Fails { Initialize-SetupDirectory }
if ($script:writes.Count) { throw 'Hostile existing setup directory was repaired instead of rejected.' }
Write-Host 'Installer identity, audio-driver isolation and protected-path tests passed.'
