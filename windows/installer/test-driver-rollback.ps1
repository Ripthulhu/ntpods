# Execute the real driver transaction with fake PnP, registry and certificate stores.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'setup-common.ps1')
$testRoot = [IO.Path]::GetFullPath((Join-Path $env:TEMP ('ntpods-driver-rollback-' + [guid]::NewGuid())))
$tempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')
if (-not $testRoot.StartsWith($tempRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe test directory.' }
$oldProgramData = $env:ProgramData
$root = Join-Path $testRoot 'dist'
$devconPath = Join-Path $root 'tools\devcon.exe'
$inboxInf = Join-Path $env:SystemRoot 'INF\microsoft_bluetooth_a2dp_src.inf'
$sid = 'S-1-5-21-123-456-789-1001'
$oldSid = 'S-1-5-21-123-456-789-1000'

function Assert-True($condition, [string]$message) {
    if (-not $condition) { throw $message }
}
function Assert-Fails([scriptblock]$body, [string]$message) {
    $failed = $false
    try { & $body } catch { $failed = $true }
    Assert-True $failed $message
}
function Assert-TestPath([string]$path) {
    $full = [IO.Path]::GetFullPath($path)
    if (-not $full.StartsWith($testRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "Test escaped its directory: $path" }
}
function Assert-TrustedInstallEntry([string]$path) { Assert-TestPath $path }
function Test-Path { param($Path, $LiteralPath, $PathType, $ErrorAction)
    if ($LiteralPath -eq $inboxInf -or $Path -eq $inboxInf) { return $true }
    Microsoft.PowerShell.Management\Test-Path @PSBoundParameters
}
function Test-MicSupported { $true }
function Start-Sleep { param($Seconds, $Milliseconds) }
function Get-WindowsDriver { param([switch]$Online, $ErrorAction) $script:packages }
function Get-NTPodsCertificates { $script:certificates }
function Get-NativeAudioBindings { $script:audioBindings }
function Get-NTPodsSigningCertificate {
    foreach ($store in 'My', 'Root', 'TrustedPublisher') {
        $script:certificates += [pscustomobject]@{ Store = $store; Thumbprint = 'NEW-CERT' }
    }
    [pscustomobject]@{ Thumbprint = 'NEW-CERT' }
}
function Set-NTPodsPackageSignature([string]$path, $cert) {
    Assert-TestPath $path
    Assert-True (Test-Path -LiteralPath $path) 'Signing input is missing.'
    Assert-True ($cert.Thumbprint -eq 'NEW-CERT') 'Unexpected signing certificate.'
    if (Fail-Once 'signing') { throw 'Injected signing failure.' }
}
function Remove-NTPodsCertificate([string]$storeName, [string]$thumbprint) {
    $script:certificates = @($script:certificates | Where-Object { $_.Store -ne $storeName -or $_.Thumbprint -ne $thumbprint })
}
function Get-ItemProperty { param($LiteralPath, $ErrorAction)
    if ($LiteralPath -notmatch '^HKLM:\\SYSTEM\\CurrentControlSet\\Services\\(NTPodsAAP|NTPodsMicACX|NTPodsAudio)\\Parameters$') { throw "Unexpected registry read: $LiteralPath" }
    [pscustomobject]@{ AllowedUserSid = $script:policy[$Matches[1]] }
}
function Set-DriverUser([string]$service, [string]$userSid) {
    Assert-UserSid $userSid
    Assert-True ($service -in 'NTPodsAAP', 'NTPodsMicACX', 'NTPodsAudio') 'Unexpected service policy write.'
    $script:policy[$service] = $userSid
}
function Remove-ItemProperty { param($LiteralPath, $Name, $ErrorAction)
    if ($Name -ne 'AllowedUserSid' -or $LiteralPath -notmatch '^HKLM:\\SYSTEM\\CurrentControlSet\\Services\\(NTPodsAAP|NTPodsMicACX|NTPodsAudio)\\Parameters$') { throw 'Unexpected registry removal.' }
    $script:policy.Remove($Matches[1])
}
function Get-PnpDevice { param([switch]$PresentOnly, $ErrorAction)
    [pscustomobject]@{ InstanceId = 'BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}\test'; HardwareID = @('AAP') }
    if ($script:micBinding) { [pscustomobject]@{ InstanceId = 'ROOT\NTPodsMicACX\test'; HardwareID = @('ROOT\NTPodsMicACX') } }
    if (-not $PresentOnly) { [pscustomobject]@{ InstanceId = 'ROOT\NTPodsMicACX\ghost'; HardwareID = @('ROOT\NTPodsMicACX') } }
}
function Get-PnpDeviceProperty { param($InstanceId, $KeyName, $ErrorAction)
    if ($InstanceId -like 'BTHENUM\*' -and $KeyName -eq 'DEVPKEY_Device_Service') {
        [pscustomobject]@{ Data = $(if ($script:aapBinding) { 'NTPodsAAP' } else { '' }) }
    } elseif ($InstanceId -eq 'ROOT\NTPodsMicACX\test' -and $KeyName -eq 'DEVPKEY_Device_DriverInfPath') {
        [pscustomobject]@{ Data = $script:micBinding }
    } elseif ($InstanceId -eq 'ROOT\NTPodsMicACX\test' -and $KeyName -eq 'DEVPKEY_Device_ProblemCode') {
        [pscustomobject]@{ Data = $script:micProblemCode }
    } else { throw "Unexpected PnP property: $InstanceId $KeyName" }
}
function Fail-Once([string]$point) {
    if ($script:faults -contains $point) {
        $script:faults = @($script:faults | Where-Object { $_ -ne $point })
        $global:LASTEXITCODE = 5
        return $true
    }
    $global:LASTEXITCODE = 0
    return $false
}
function Add-FakePackage([string]$inf) {
    Assert-TestPath $inf
    $name = Split-Path -Leaf $inf
    Assert-True ($name -in 'NTPodsAAP.inf', 'AudioCodec.inf', 'NTPodsAudio.inf') 'Attempted to stage another driver.'
    $payload = (Get-Content -LiteralPath $inf -Raw).Trim()
    $existing = @($script:packages | Where-Object Payload -eq $payload)
    if ($existing.Count) { return $existing[0].Driver }
    $published = 'oem' + $script:nextPackage++ + '.inf'
    $script:packages += [pscustomobject]@{ Driver = $published; OriginalFileName = $name; ProviderName = 'NTPods'; Payload = $payload }
    $published
}
function pnputil {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments)
    $script:calls += 'pnputil:' + ($Arguments -join '|')
    $global:LASTEXITCODE = 0
    switch ($Arguments[0]) {
        '/export-driver' {
            $package = @($script:packages | Where-Object Driver -eq $Arguments[1])
            Assert-True ($package.Count -eq 1 -and $package[0].ProviderName -eq 'NTPods') 'Export selected an unrelated driver.'
            if (Fail-Once 'export') { return }
            Assert-TestPath $Arguments[2]
            Set-Content -LiteralPath (Join-Path $Arguments[2] $package[0].OriginalFileName) -Value $package[0].Payload
        }
        '/add-driver' {
            Assert-TestPath $Arguments[1]
            $payload = (Get-Content -LiteralPath $Arguments[1] -Raw).Trim()
            $point = if ($payload -eq 'new-aap') { if ($Arguments -contains '/install') { 'install-new-aap' } else { 'stage-aap' } }
                elseif ($payload -eq 'new-mic') { 'stage-mic' }
                elseif ($payload -eq 'new-audio') { 'stage-audio' }
                elseif ($payload -eq 'old-aap') { 'restore-aap' }
                else { 'restore-mic' }
            if (Fail-Once $point) { return }
            $published = Add-FakePackage $Arguments[1]
            if ($Arguments -contains '/install') { $script:aapBinding = $published }
            $global:LASTEXITCODE = $script:pnputilInstallExitCode
        }
        '/delete-driver' {
            $package = @($script:packages | Where-Object Driver -eq $Arguments[1])
            Assert-True ($package.Count -eq 1 -and $package[0].ProviderName -eq 'NTPods') 'Removal selected an unrelated driver.'
            if ($package[0].Payload -eq 'old-aap' -and (Fail-Once 'delete-old-aap')) { return }
            if ($script:aapBinding -eq $Arguments[1]) { $script:aapBinding = $null }
            if ($script:micBinding -eq $Arguments[1]) { $script:micBinding = $null }
            $script:packages = @($script:packages | Where-Object Driver -ne $Arguments[1])
        }
        default { throw 'Unexpected pnputil command.' }
    }
}

function Reset-Fixture([string[]]$failures) {
    if (Test-Path -LiteralPath (Get-DriverCheckpointPath)) { throw 'Previous test left a driver backup.' }
    $script:packages = @(
        [pscustomobject]@{ Driver = 'oem1.inf'; OriginalFileName = 'NTPodsAAP.inf'; ProviderName = 'NTPods'; Payload = 'old-aap' },
        [pscustomobject]@{ Driver = 'oem2.inf'; OriginalFileName = 'AudioCodec.inf'; ProviderName = 'NTPods'; Payload = 'old-mic' },
        [pscustomobject]@{ Driver = 'oem90.inf'; OriginalFileName = 'AlternativeA2DP.inf'; ProviderName = 'Luculent'; Payload = 'a2dp' },
        [pscustomobject]@{ Driver = 'oem91.inf'; OriginalFileName = 'AudioCodec.inf'; ProviderName = 'Other vendor'; Payload = 'other-mic' }
    )
    $script:certificates = @([pscustomobject]@{ Store = 'Root'; Thumbprint = 'OLD-CERT' })
    $script:policy = @{ NTPodsAAP = $oldSid; NTPodsMicACX = $oldSid }
    $script:nextPackage = 3
    $script:aapBinding = 'oem1.inf'; $script:micBinding = 'oem2.inf'
    $script:micProblemCode = 0; $script:newMicProblemCode = 0
    $script:micInstallExitCode = 0; $script:pnputilInstallExitCode = 0
    $script:calls = @(); $script:faults = @($failures)
    $script:audioBindings = @()
}
function Assert-Restored {
    Assert-True (($script:packages.Payload | Sort-Object) -join ',' -ceq 'a2dp,old-aap,old-mic,other-mic') 'Previous packages were not restored exactly.'
    Assert-True (($script:packages | Where-Object Driver -eq $script:aapBinding).Payload -eq 'old-aap') 'AAP did not return to the previous driver.'
    Assert-True (($script:packages | Where-Object Driver -eq $script:micBinding).Payload -eq 'old-mic') 'Microphone did not return to the previous driver.'
    Assert-True ($script:micProblemCode -eq 0) 'Microphone retained the failed replacement status.'
    Assert-True ($script:policy.NTPodsAAP -eq $oldSid -and $script:policy.NTPodsMicACX -eq $oldSid) 'Driver SID policy was not restored.'
    Assert-True ($script:certificates.Count -eq 1 -and $script:certificates[0].Thumbprint -eq 'OLD-CERT') 'Rollback removed an old certificate or kept a new one.'
    Assert-True (-not (Test-Path -LiteralPath (Get-DriverCheckpointPath))) 'Successful rollback left a backup.'
}

function Reset-AudioUpgradeFixture([bool]$existingManifest, [string[]]$failures) {
    Reset-Fixture $failures
    $path = Get-NativeAudioBackupPath
    Assert-TestPath $path
    New-Item -ItemType Directory -Path $path -Force | Out-Null
    $manifest = Join-Path $path 'bindings.json'
    if (Test-Path -LiteralPath $manifest) { Remove-Item -LiteralPath $manifest -Force }
    if ($existingManifest) {
        Save-NativeAudioJson $manifest @([pscustomobject]@{
            InstanceId = 'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&200E\previous'
            Published = 'microsoft_bluetooth_a2dp_src.inf'; Inf = $null
        })
    }
    # This paired Pro 2 is absent, so setup saves its provider without binding it.
    $script:audioBindings = @([pscustomobject]@{
        InstanceId = 'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2014\new-model'
        Published = 'microsoft_bluetooth_a2dp_src.inf'; Service = 'BthA2dp'
    })
}

function Read-LegacyAudioBackup {
    # The preceding release accepted only these two model IDs.
    function Assert-NativeAudioInstance([string]$instanceId) {
        if ($instanceId -notmatch '\ABTHENUM\\\{0000110b-0000-1000-8000-00805f9b34fb\}_VID&0001004C_PID&(200E|2027)\\[^\\\x00-\x20]+\z') {
            throw 'Invalid AirPods playback instance.'
        }
    }
    Read-NativeAudioBackup
}

try {
    $env:ProgramData = Join-Path $testRoot 'ProgramData'
    foreach ($folder in @((Join-Path $env:ProgramData 'NTPods'), (Join-Path $root 'driver'), (Join-Path $root 'driver-mic-acx'), (Join-Path $root 'driver-audio'), (Join-Path $root 'tools'))) {
        New-Item -ItemType Directory -Path $folder -Force | Out-Null
    }
    foreach ($file in 'NTPodsAAP.inf', 'NTPodsAAP.sys', 'ntpodsaap.cat') { Set-Content -LiteralPath (Join-Path $root "driver\$file") -Value 'new-aap' }
    foreach ($file in 'AudioCodec.inf', 'AudioCodec.sys', 'audiocodec.cat') { Set-Content -LiteralPath (Join-Path $root "driver-mic-acx\$file") -Value 'new-mic' }
    foreach ($file in 'NTPodsAudio.inf', 'NTPodsAudio.sys', 'ntpodsaudio.cat') { Set-Content -LiteralPath (Join-Path $root "driver-audio\$file") -Value 'new-audio' }
    Set-Item -LiteralPath ('Function:\' + $devconPath) -Value {
        param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments)
        $script:calls += 'devcon:' + ($Arguments -join '|')
        $global:LASTEXITCODE = 0
        if ($Arguments[0] -eq 'remove') {
            Assert-True ($Arguments[1] -in 'ROOT\NTPodsMicACX', 'ROOT\NTPodsMicPC') 'Removal selected another device.'
            if (Fail-Once 'remove-mic') { return }
            if ($Arguments[1] -eq 'ROOT\NTPodsMicACX') { $script:micBinding = $null }
        } elseif ($Arguments[0] -eq 'install') {
            Assert-True ($Arguments[2] -eq 'ROOT\NTPodsMicACX') 'Installation selected another device.'
            $payload = (Get-Content -LiteralPath $Arguments[1] -Raw).Trim()
            if ($payload -eq 'new-mic' -and (Fail-Once 'install-new-mic')) { return }
            if ($payload -eq 'new-mic' -and (Fail-Once 'ghost-mic')) { $global:LASTEXITCODE = 0; return }
            $script:micBinding = Add-FakePackage $Arguments[1]
            $script:micProblemCode = 0
            if ($payload -eq 'new-mic') {
                $script:micProblemCode = $script:newMicProblemCode
                if (Fail-Once 'failed-start-mic') { $script:micProblemCode = 10 }
                if (Fail-Once 'failed-start-mic-reboot') { $script:micProblemCode = 10; $script:micInstallExitCode = 1 }
                if (Fail-Once 'missing-mic-status') { $script:micProblemCode = $null }
                $global:LASTEXITCODE = $script:micInstallExitCode
            }
        } else { throw 'Unexpected devcon command.' }
    }

    foreach ($failure in 'export', 'stage-aap', 'stage-mic', 'stage-audio', 'delete-old-aap', 'install-new-aap', 'remove-mic', 'install-new-mic', 'ghost-mic', 'failed-start-mic', 'failed-start-mic-reboot', 'missing-mic-status') {
        Reset-Fixture @($failure)
        Assert-Fails { Install-NTPodsDrivers $root (Join-Path $testRoot $failure) 'acx' $sid $failure } "Fault did not fail installation: $failure"
        Assert-True ($script:faults.Count -eq 0) "Fault was not reached: $failure"
        Assert-Restored
        if ($failure -in 'export', 'stage-aap', 'stage-mic', 'stage-audio') {
            Assert-True (-not @($script:calls | Where-Object { $_ -like 'devcon:*' -or $_ -like 'pnputil:/delete-driver|oem1.inf*' -or $_ -like 'pnputil:/delete-driver|oem2.inf*' }).Count) 'Staging failure disturbed an installed driver.'
        }
        Write-Host "PASS recovery after $failure"
    }

    foreach ($devconExit in 0, 1) {
        Reset-Fixture @()
        $script:newMicProblemCode = 14
        $script:micInstallExitCode = $devconExit
        $script:pnputilInstallExitCode = 3010
        $transaction = "restart-required-$devconExit"
        Install-NTPodsDrivers $root (Join-Path $testRoot $transaction) 'acx' $sid $transaction
        Assert-True ($script:micProblemCode -eq 14) 'Reboot-required status was not reached.'
        Assert-True (($script:packages | Where-Object Driver -eq $script:micBinding).Payload -eq 'new-mic') 'A legitimate reboot requirement rolled back the new driver.'
        Complete-NTPodsDriverTransaction $transaction
        Write-Host "PASS reboot-required microphone with DevCon exit $devconExit and PnPUtil exit 3010"
    }

    Reset-Fixture @()
    Install-NTPodsDrivers $root (Join-Path $testRoot 'later-action') 'acx' $sid 'later-action'
    Assert-True ((Read-NTPodsDriverCheckpoint 'later-action').Devices.Count -eq 1) 'Checkpoint included a ghost microphone.'
    Restore-NTPodsDriverCheckpoint $root 'later-action'
    Assert-Restored
    Write-Host 'PASS rollback after a later setup action fails'

    Reset-Fixture @('install-new-mic', 'restore-aap')
    Assert-Fails { Install-NTPodsDrivers $root (Join-Path $testRoot 'recovery-failure') 'acx' $sid 'recovery-failure' } 'Expected recovery failure.'
    Assert-True ($script:faults.Count -eq 0) 'Recovery failure was not reached.'
    Assert-True (Test-Path -LiteralPath (Join-Path (Get-DriverCheckpointPath) 'checkpoint.json')) 'Failed recovery discarded its backup.'
    Restore-NTPodsDriverCheckpoint $root 'recovery-failure'
    Assert-Restored
    Write-Host 'PASS failed recovery retains a usable backup'

    Reset-Fixture @()
    Save-NTPodsDriverCheckpoint 'Owner'
    $before = $script:calls.Count
    Assert-Fails { Save-NTPodsDriverCheckpoint 'Other' } 'A new transaction replaced an existing backup.'
    Assert-Fails { Restore-NTPodsDriverCheckpoint $root 'owner' } 'Restore ignored ordinal transaction ownership.'
    Assert-Fails { Complete-NTPodsDriverTransaction 'Other' } 'Commit deleted another transaction backup.'
    Assert-Fails { Complete-NTPodsDriverTransaction '' } 'Commit accepted a missing transaction ID.'
    Assert-True ($script:calls.Count -eq $before) 'Ownership rejection touched a driver.'
    Assert-True (Test-Path -LiteralPath (Join-Path (Get-DriverCheckpointPath) 'checkpoint.json')) 'Ownership rejection discarded its backup.'
    Complete-NTPodsDriverTransaction 'Owner'
    Write-Host 'PASS stale and mismatched transaction backups are preserved'

    Reset-Fixture @()
    $script:packages = @($script:packages | Where-Object ProviderName -ne 'NTPods')
    $script:aapBinding = $script:micBinding = $null
    $script:policy = @{}
    $script:certificates = @()
    Install-NTPodsDrivers $root (Join-Path $testRoot 'fresh') 'acx' $sid 'fresh'
    Restore-NTPodsDriverCheckpoint $root 'fresh'
    Assert-True (($script:packages.Payload | Sort-Object) -join ',' -ceq 'a2dp,other-mic') 'Fresh rollback removed another driver or kept a new package.'
    Assert-True (-not $script:aapBinding -and -not $script:micBinding) 'Fresh rollback kept a driver binding.'
    Assert-True ($script:policy.Count -eq 0 -and $script:certificates.Count -eq 0) 'Fresh rollback kept SID policy or certificates.'
    Assert-True (-not (Test-Path -LiteralPath (Get-DriverCheckpointPath))) 'Fresh rollback left a backup.'
    Write-Host 'PASS fresh installation rollback'

    Reset-Fixture @()
    Install-NTPodsDrivers $root (Join-Path $testRoot 'commit') 'acx' $sid 'commit'
    $before = $script:calls.Count
    Complete-NTPodsDriverTransaction 'commit'
    Assert-True ($script:calls.Count -eq $before) 'Commit touched a driver.'
    Assert-True (($script:packages.Payload | Sort-Object) -join ',' -ceq 'a2dp,new-aap,new-audio,new-mic,other-mic') 'Commit changed the installed packages.'
    Assert-True (-not (Test-Path -LiteralPath (Get-DriverCheckpointPath))) 'Commit left the backup.'
    Write-Host 'PASS commit removes only its own backup'

    foreach ($existingManifest in $true, $false) {
        Reset-AudioUpgradeFixture $existingManifest @('signing')
        $transaction = "audio-signing-$existingManifest"
        Assert-Fails { Install-NTPodsDrivers $root (Join-Path $testRoot $transaction) 'acx' $sid $transaction } 'Signing failure did not stop the upgrade.'
        Assert-True ($script:faults.Count -eq 0) 'Signing failure was not reached.'
        Assert-Restored
        Assert-True ((Test-Path -LiteralPath (Join-Path (Get-NativeAudioBackupPath) 'bindings.json')) -eq $existingManifest) 'Rollback changed whether an original-provider manifest existed.'
        Assert-True (@(Read-LegacyAudioBackup).Count -eq [int]$existingManifest) 'The previous installer cannot read the restored manifest.'
        Restore-NTPodsDriverCheckpoint $root $transaction
        Assert-True (@(Read-LegacyAudioBackup).Count -eq [int]$existingManifest) 'Repeated rollback changed the restored manifest.'
    }
    Write-Host 'PASS failed model expansion restores the previous manifest or its absence'

    Reset-AudioUpgradeFixture $true @('install-new-mic', 'restore-aap')
    Assert-Fails { Install-NTPodsDrivers $root (Join-Path $testRoot 'audio-recovery') 'acx' $sid 'audio-recovery' } 'Expected upgrade recovery failure.'
    Assert-True ($script:faults.Count -eq 0) 'Upgrade recovery failure was not reached.'
    Assert-True (@(Read-NativeAudioBackup).Count -eq 2) 'Failed recovery discarded the new model backup.'
    Assert-Fails { Read-LegacyAudioBackup } 'The fixture did not reproduce prior-version rejection.'
    Restore-NTPodsDriverCheckpoint $root 'audio-recovery'
    Assert-Restored
    Assert-True (@(Read-LegacyAudioBackup).Count -eq 1) 'Recovery retry did not restore the previous manifest.'
    Write-Host 'PASS failed recovery retains expanded records until recovery succeeds'

    Reset-AudioUpgradeFixture $true @()
    $manifest = Join-Path (Get-NativeAudioBackupPath) 'bindings.json'
    Set-Content -LiteralPath $manifest -Value '[{"InstanceId":"ROOT\\OTHER\\test","Published":"oem90.inf","Inf":null}]'
    Assert-Fails { Install-NTPodsDrivers $root (Join-Path $testRoot 'audio-invalid-source') 'acx' $sid 'audio-invalid-source' } 'Invalid original-provider data was checkpointed.'
    Assert-Restored
    Assert-True ((Get-Content -LiteralPath $manifest -Raw) -match 'ROOT') 'Failed checkpoint creation overwrote the invalid manifest.'
    Write-Host 'PASS invalid original-provider data leaves no partial checkpoint'

    Reset-AudioUpgradeFixture $true @()
    Save-NTPodsDriverCheckpoint 'audio-invalid-checkpoint'
    $checkpoint = Join-Path (Get-DriverCheckpointPath) 'checkpoint.json'
    $originalCheckpoint = Get-Content -LiteralPath $checkpoint -Raw
    $before = $script:calls.Count
    foreach ($invalid in @(
        { param($saved) $saved.AudioOriginalsPresent = 'true' },
        { param($saved) $saved.AudioOriginalsPresent = $false },
        { param($saved) $saved.AudioOriginals = $null },
        { param($saved) $saved.AudioOriginals += $saved.AudioOriginals[0] },
        { param($saved) $saved.AudioOriginals[0].Inf = '..\outside.inf' }
    )) {
        $saved = $originalCheckpoint | ConvertFrom-Json
        & $invalid $saved
        $saved | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $checkpoint -Encoding UTF8
        Assert-Fails { Restore-NTPodsDriverCheckpoint $root 'audio-invalid-checkpoint' } 'Malformed original-provider checkpoint reached recovery.'
        Assert-True ($script:calls.Count -eq $before) 'Malformed original-provider checkpoint touched a driver.'
        Assert-True (@(Read-LegacyAudioBackup).Count -eq 1) 'Malformed checkpoint replaced the original-provider manifest.'
    }
    Set-Content -LiteralPath $checkpoint -Value $originalCheckpoint -Encoding UTF8
    Complete-NTPodsDriverTransaction 'audio-invalid-checkpoint'
    Write-Host 'PASS malformed original-provider checkpoints fail before recovery'

    Reset-AudioUpgradeFixture $true @()
    Install-NTPodsDrivers $root (Join-Path $testRoot 'audio-commit') 'acx' $sid 'audio-commit'
    Complete-NTPodsDriverTransaction 'audio-commit'
    Assert-True (@(Read-NativeAudioBackup).Count -eq 2) 'Commit discarded the new model backup.'
    Assert-Fails { Read-LegacyAudioBackup } 'Commit restored the previous manifest instead of keeping the expanded one.'
    Write-Host 'PASS successful model expansion keeps its original-provider records'
} finally {
    Remove-Item -LiteralPath ('Function:\' + $devconPath) -ErrorAction SilentlyContinue
    $env:ProgramData = $oldProgramData
    if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
