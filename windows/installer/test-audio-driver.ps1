# Run the AudioSink install and restore functions against temporary driver and PnP stores.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'setup-common.ps1')
$testRoot = [IO.Path]::GetFullPath((Join-Path $env:TEMP ('ntpods-audio-driver-' + [guid]::NewGuid())))
$tempRoot = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\')
if (-not $testRoot.StartsWith($tempRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe test directory.' }
$oldProgramData = $env:ProgramData
$inboxInf = Join-Path $env:SystemRoot 'INF\microsoft_bluetooth_a2dp_src.inf'
$root = Join-Path $testRoot 'dist'
$helper = Join-Path $root 'tools\audio-binding.exe'
$audioInf = Join-Path $root 'driver-audio\NTPodsAudio.inf'
$pro1 = 'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&200E'
$pro3 = 'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2027'
$products = @('2002', '200A', '200E', '200F', '2013', '2014', '2019', '201B', '201F', '2024', '2027')
$supportedIds = @($products | ForEach-Object { 'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&' + $_ })

function Assert-True($condition, [string]$message) { if (-not $condition) { throw $message } }
function Assert-Fails([scriptblock]$body, [string]$message) {
    $failed = $false
    try { & $body } catch { $failed = $true }
    Assert-True $failed $message
}
function Assert-TestPath([string]$path) {
    if (-not [IO.Path]::GetFullPath($path).StartsWith($testRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "Test escaped its directory: $path" }
}
function Assert-TrustedInstallEntry([string]$path) { Assert-TestPath $path }
function Test-Path { param($Path, $LiteralPath, $PathType, $ErrorAction)
    if ($LiteralPath -eq $inboxInf -or $Path -eq $inboxInf) { return $true }
    Microsoft.PowerShell.Management\Test-Path @PSBoundParameters
}
function Get-WindowsDriver { param([switch]$Online, $ErrorAction) $script:packages }
function Get-PnpDevice { param([switch]$PresentOnly, $ErrorAction)
    @($script:devices | Where-Object { -not $PresentOnly -or $_.Present })
}
function Get-PnpDeviceProperty { param($InstanceId, $KeyName, $ErrorAction)
    $device = @($script:devices | Where-Object InstanceId -eq $InstanceId)
    Assert-True ($device.Count -eq 1) 'Unexpected PnP instance.'
    $data = switch ($KeyName) {
        'DEVPKEY_Device_DriverInfPath' { $device[0].Published }
        'DEVPKEY_Device_Service' { $device[0].Service }
        'DEVPKEY_Device_ProblemCode' { $device[0].Problem }
        default { throw 'Unexpected PnP property.' }
    }
    [pscustomobject]@{ Data = $data }
}
function Add-Package([string]$published, [string]$name, [string]$service, [string]$payload) {
    $script:packages += [pscustomobject]@{ Driver = $published; OriginalFileName = $name; ProviderName = $(if ($service -eq 'NTPodsAudio') { 'NTPods' } else { 'Other vendor' }); Service = $service; Payload = $payload }
}
function Package-ForInf([string]$inf) {
    if ($inf -eq $inboxInf) { return [pscustomobject]@{ Driver = 'microsoft_bluetooth_a2dp_src.inf'; Service = 'BthA2dp'; Payload = 'inbox' } }
    Assert-TestPath $inf
    $payload = (Get-Content -LiteralPath $inf -Raw).Trim()
    $found = @($script:packages | Where-Object Payload -eq $payload)
    Assert-True ($found.Count -eq 1) "Driver package is not staged: $payload"
    $found[0]
}
function pnputil {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments)
    $script:calls += 'pnputil:' + ($Arguments -join '|')
    $global:LASTEXITCODE = 0
    if ($script:failOperation -eq $Arguments[0]) { $script:failOperation = ''; $global:LASTEXITCODE = 5; return }
    switch ($Arguments[0]) {
        '/export-driver' {
            $package = @($script:packages | Where-Object Driver -eq $Arguments[1])
            Assert-True ($package.Count -eq 1) 'Export selected an unknown package.'
            Assert-TestPath $Arguments[2]
            Set-Content -LiteralPath (Join-Path $Arguments[2] $package[0].OriginalFileName) -Value $package[0].Payload
        }
        '/add-driver' { Package-ForInf $Arguments[1] | Out-Null }
        '/delete-driver' {
            $package = @($script:packages | Where-Object Driver -eq $Arguments[1])
            Assert-True ($package.Count -eq 1 -and $package[0].Service -eq 'NTPodsAudio') 'Removal selected an unrelated package.'
            $script:packages = @($script:packages | Where-Object Driver -ne $Arguments[1])
            foreach ($device in $script:devices | Where-Object Published -eq $Arguments[1]) { $device.Service = ''; $device.Published = '' }
        }
        default { throw 'Unexpected pnputil command.' }
    }
}
function Reset-Fixture([string]$name) {
    $env:ProgramData = Join-Path $testRoot $name
    New-Item -ItemType Directory -Path (Join-Path $env:ProgramData 'NTPods') -Force | Out-Null
    $script:packages = @()
    Add-Package 'oem90.inf' 'AlternativeA2DP.inf' 'AltA2DP' 'alternative'
    Add-Package 'oem91.inf' 'Unrelated.inf' 'OtherAudio' 'unrelated'
    Add-Package 'oem1.inf' 'NTPodsAudio.inf' 'NTPodsAudio' 'new-audio'
    $script:devices = @(
        [pscustomobject]@{ InstanceId = "$pro1\first"; HardwareID = @('BTHENUM\generic', $pro1); Present = $true; Published = 'oem90.inf'; Service = 'AltA2DP'; Problem = 0 },
        [pscustomobject]@{ InstanceId = "$pro3\second"; HardwareID = @($pro3.ToLowerInvariant()); Present = $true; Published = 'microsoft_bluetooth_a2dp_src.inf'; Service = 'BthA2dp'; Problem = 0 },
        [pscustomobject]@{ InstanceId = "$pro1\absent"; HardwareID = @($pro1); Present = $false; Published = 'oem90.inf'; Service = 'AltA2DP'; Problem = 0 },
        [pscustomobject]@{ InstanceId = "$pro3-extra\other"; HardwareID = @("$pro3-extra"); Present = $true; Published = 'oem91.inf'; Service = 'OtherAudio'; Problem = 0 },
        [pscustomobject]@{ InstanceId = 'BTHENUM\generic\other'; HardwareID = @('BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}'); Present = $true; Published = 'oem91.inf'; Service = 'OtherAudio'; Problem = 0 },
        [pscustomobject]@{ InstanceId = 'BTHENUM\handsfree\other'; HardwareID = @($pro3.Replace('0000110b', '0000111e')); Present = $true; Published = 'oem91.inf'; Service = 'OtherAudio'; Problem = 0 }
    )
    $script:calls = @(); $script:failOperation = ''; $script:failBind = $false; $script:requireBackup = $false
}
function Write-Backup($entries) {
    $path = Get-NativeAudioBackupPath
    New-Item -ItemType Directory -Path $path -Force | Out-Null
    ConvertTo-Json -InputObject @($entries) -Depth 4 | Set-Content -LiteralPath (Join-Path $path 'bindings.json') -Encoding UTF8
}

try {
    New-Item -ItemType Directory -Path (Split-Path $helper), (Split-Path $audioInf) -Force | Out-Null
    Set-Content -LiteralPath $helper -Value 'No executable: calls are mocked.'
    Set-Content -LiteralPath $audioInf -Value 'new-audio'
    Set-Item -LiteralPath ('Function:\' + $helper) -Value {
        param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments)
        Assert-True ($Arguments[0] -in '--bind', '--check') 'Unexpected audio helper operation.'
        $device = @($script:devices | Where-Object InstanceId -eq $Arguments[1])
        Assert-True ($device.Count -eq 1 -and @($device[0].HardwareID | Where-Object { $_ -in $supportedIds }).Count -gt 0) 'Binding selected an unrelated device.'
        if ($script:requireBackup) {
            Assert-True (@(Read-NativeAudioBackup | Where-Object InstanceId -eq $device[0].InstanceId).Count -eq 1) 'Binding happened before the original driver was saved.'
        }
        $script:calls += $(if ($Arguments[0] -eq '--bind') { 'bind:' } else { 'check:' }) + ($Arguments -join '|')
        $global:LASTEXITCODE = 0
        if ($Arguments[0] -eq '--check') { Package-ForInf $Arguments[2] | Out-Null; return }
        if ($script:failBind) { $script:failBind = $false; $global:LASTEXITCODE = 5; return }
        $package = Package-ForInf $Arguments[2]
        $device[0].Service = $package.Service; $device[0].Published = $package.Driver
    }

    $inf = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\drivers\audio\NTPodsAudio.inf') -Raw
    $infIds = @([regex]::Matches($inf, '(?m)^%Device%=Audio,([^\r\n]+)') | ForEach-Object { $_.Groups[1].Value })
    $bindingSource = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'audio-binding.cpp') -Raw
    $bindingList = [regex]::Match($bindingSource, '(?s)supported_ids\[\]\s*=\s*\{(.*?)\};').Groups[1].Value
    $bindingIds = @([regex]::Matches($bindingList, 'L"([^"]+)"') | ForEach-Object { $_.Groups[1].Value.Replace('\\', '\') })
    $scriptIds = @([regex]::Matches((Get-Command Get-NativeAudioDevices).Definition, "'([^']+)'" ) | ForEach-Object { $_.Groups[1].Value })
    $expectedIds = ($supportedIds | Sort-Object) -join "`n"
    foreach ($list in @(@{ Name = 'INF'; Ids = $infIds }, @{ Name = 'helper'; Ids = $bindingIds }, @{ Name = 'installer'; Ids = $scriptIds })) {
        Assert-True ((@($list.Ids | Sort-Object) -join "`n") -ieq $expectedIds) "$($list.Name) playback targets differ from the tested models."
    }
    Write-Host 'PASS INF, helper and installer target lists match'

    foreach ($id in $supportedIds) {
        Reset-Fixture ('model-' + $id.Substring($id.Length - 4))
        $script:devices = @(
            [pscustomobject]@{ InstanceId = "$id\external"; HardwareID = @($id.ToLowerInvariant()); Present = $true; Published = 'oem90.inf'; Service = 'AltA2DP'; Problem = 0 },
            [pscustomobject]@{ InstanceId = "$id\inbox"; HardwareID = @($id); Present = $true; Published = 'microsoft_bluetooth_a2dp_src.inf'; Service = 'BthA2dp'; Problem = 0 },
            [pscustomobject]@{ InstanceId = "$id\absent"; HardwareID = @($id); Present = $false; Published = 'oem90.inf'; Service = 'AltA2DP'; Problem = 0 }
        )
        Assert-NativeAudioInstance "$($id.ToLowerInvariant())\lowercase"
        Assert-True (@(Get-NativeAudioDevices).Count -eq 3) "A supported model was omitted: $id"
        Save-NativeAudioOriginals
        Assert-True (@(Read-NativeAudioBackup).Count -eq 3) "Original providers were not saved for $id"
        $script:requireBackup = $true
        Install-NativeAudioDriver $root $audioInf
        Assert-True ($script:devices[0].Service -eq 'NTPodsAudio' -and $script:devices[1].Service -eq 'NTPodsAudio') "A present model did not bind: $id"
        Assert-True ($script:devices[2].Service -eq 'AltA2DP') 'Install bound an absent device.'
        Save-NativeAudioOriginals
        Assert-True (@(Read-NativeAudioBackup | Where-Object Published -eq 'oem90.inf').Count -eq 2) 'Upgrade replaced the original external provider.'
        $script:requireBackup = $false
        $script:devices[2].Service = 'NTPodsAudio'; $script:devices[2].Published = 'oem1.inf'
        Uninstall-NativeAudioDriver $root
        Assert-True ($script:devices[0].Service -eq 'AltA2DP' -and $script:devices[1].Service -eq 'BthA2dp') "Uninstall lost the original providers for $id"
        Assert-True (Test-Path -LiteralPath (Get-NativeAudioBackupPath)) 'Absent device lost its original-provider backup.'
        $script:devices[2].Present = $true
        Uninstall-NativeAudioDriver $root
        Assert-True ($script:devices[2].Service -eq 'AltA2DP') 'Reconnect did not restore the absent device.'
        Assert-True (-not (Test-Path -LiteralPath (Get-NativeAudioBackupPath))) 'Completed restore retained its backup.'

        Reset-Fixture ('unbacked-' + $id.Substring($id.Length - 4))
        $script:devices = @([pscustomobject]@{ InstanceId = "$id\later-paired"; HardwareID = @($id); Present = $true; Published = 'oem1.inf'; Service = 'NTPodsAudio'; Problem = 0 })
        Uninstall-NativeAudioDriver $root
        Assert-True ($script:devices[0].Service -eq 'BthA2dp') "A later-paired model did not return to Windows playback: $id"
    }
    Write-Host "PASS backup, install, upgrade and restore for all $($supportedIds.Count) model IDs"

    Reset-Fixture 'invalid-targets'
    $invalidIds = @(
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}',
        $pro3.Replace('0000110b', '0000111e'),
        $pro3.Replace('0001004C', '00010000'),
        $pro3.Replace('0001004C', '0002004C'),
        $pro3.Replace('2027', 'FFFF'),
        $pro3.Replace('2027', '200B'),
        ($pro3 + '-extra')
    )
    $script:devices = @($invalidIds | ForEach-Object { [pscustomobject]@{ InstanceId = "$_\other"; HardwareID = @($_); Present = $true; Published = 'oem91.inf'; Service = 'OtherAudio'; Problem = 0 } })
    Assert-True (@(Get-NativeAudioDevices).Count -eq 0) 'An unsupported hardware ID was accepted.'
    foreach ($badId in $invalidIds) { Assert-Fails { Assert-NativeAudioInstance "$badId\other" } 'An unsupported saved instance was accepted.' }
    foreach ($suffix in @('', 'two\parts', 'has space', "has`ttab", "has`nnewline", "has$([char]0)null")) {
        Assert-Fails { Assert-NativeAudioInstance "$pro3\$suffix" } 'An invalid instance suffix was accepted.'
    }
    Save-NativeAudioOriginals
    Install-NativeAudioDriver $root $audioInf
    Assert-True ($script:calls.Count -eq 0) 'An unsupported device reached a driver operation.'
    Write-Host 'PASS unrelated services, vendors, products and invalid suffixes are rejected'

    Reset-Fixture 'install-uninstall'
    Assert-True (@(Get-NativeAudioDevices).Count -eq 3) 'Hardware-ID matching included another device or omitted a supported one.'
    Save-NativeAudioOriginals
    $saved = @(Read-NativeAudioBackup)
    Assert-True ($saved.Count -eq 3 -and @($script:calls | Where-Object { $_ -like 'pnputil:/export-driver|*' }).Count -eq 1) 'Original packages were omitted or exported twice.'
    Save-NativeAudioOriginals
    Assert-True (@(Read-NativeAudioBackup).Count -eq 3) 'Repeated backup duplicated entries.'
    $script:requireBackup = $true
    Install-NativeAudioDriver $root $audioInf
    Assert-True (@($script:calls | Where-Object { $_ -like 'bind:*' }).Count -eq 2) 'Install must bind exactly the two present supported instances.'
    Assert-True ($script:devices[2].Service -eq 'AltA2DP' -and $script:devices[3].Service -eq 'OtherAudio') 'Install changed an absent or unrelated device.'
    $manifest = Join-Path (Get-NativeAudioBackupPath) 'bindings.json'
    $originalManifest = Get-Content -LiteralPath $manifest -Raw
    Save-NativeAudioOriginals
    Assert-True ((Get-Content -LiteralPath $manifest -Raw) -ceq $originalManifest) 'An upgrade replaced the original provider with NTPods.'
    $script:requireBackup = $false
    Uninstall-NativeAudioDriver $root
    Assert-True ($script:devices[0].Service -eq 'AltA2DP' -and $script:devices[1].Service -eq 'BthA2dp') 'Uninstall did not restore the original playback providers.'
    Assert-True ($script:packages.Driver -contains 'oem90.inf' -and $script:packages.Driver -contains 'oem91.inf') 'Uninstall removed an unrelated driver package.'
    Assert-True (-not (Test-Path -LiteralPath (Get-NativeAudioBackupPath))) 'Successful uninstall kept the original-driver backup.'
    Write-Host 'PASS original backup, exact targeting, repeated backup and uninstall restore'

    Reset-Fixture 'failed-export'
    $script:failOperation = '/export-driver'
    Assert-Fails { Save-NativeAudioOriginals } 'Failed export must not produce a completed backup.'
    Assert-True (-not @($script:calls | Where-Object { $_ -like 'bind:*' }).Count) 'Export failure changed a device.'
    Save-NativeAudioOriginals
    Assert-True (@(Read-NativeAudioBackup).Count -eq 3) 'A failed export left an unusable partial folder.'
    Write-Host 'PASS export failure can be retried without binding a device'

    Reset-Fixture 'fresh'
    $script:devices[0].Service = 'NTPodsAudio'; $script:devices[0].Published = 'oem1.inf'
    Uninstall-NativeAudioDriver $root
    Assert-True ($script:devices[0].Service -eq 'BthA2dp') 'A device without an earlier provider must return to the inbox driver.'
    Write-Host 'PASS inbox fallback without an original backup'

    Reset-Fixture 'stage-failure'
    Save-NativeAudioOriginals
    Install-NativeAudioDriver $root $audioInf
    $script:calls = @(); $script:failOperation = '/add-driver'
    Assert-Fails { Uninstall-NativeAudioDriver $root } 'Original-driver staging failure did not stop uninstall.'
    Assert-True (-not @($script:calls | Where-Object { $_ -like 'pnputil:/delete-driver|*' -or $_ -like 'bind:*' }).Count) 'Staging failure disturbed the active driver.'
    Assert-True ($script:devices[0].Service -eq 'NTPodsAudio' -and $script:packages.Driver -contains 'oem1.inf') 'Staging failure removed working playback.'
    Write-Host 'PASS restore preflight failure preserves the active driver'

    Reset-Fixture 'upgrade-rollback'
    $checkpoint = Join-Path $env:ProgramData 'NTPods\driver-rollback'
    New-Item -ItemType Directory -Path (Join-Path $checkpoint 'oem2.inf') -Force | Out-Null
    $previousInf = Join-Path $checkpoint 'oem2.inf\NTPodsAudio.inf'
    Set-Content -LiteralPath $previousInf -Value 'old-audio'
    Add-Package 'oem2.inf' 'NTPodsAudio.inf' 'NTPodsAudio' 'old-audio'
    $bindings = @([pscustomobject]@{ InstanceId = $script:devices[0].InstanceId; Published = 'oem2.inf'; Service = 'NTPodsAudio' })
    $checkpointPackages = @([pscustomobject]@{ Published = 'oem2.inf'; Name = 'NTPodsAudio.inf'; Inf = 'oem2.inf\NTPodsAudio.inf' })
    Restore-NativeAudioBindings $root $bindings $checkpointPackages $checkpoint
    Assert-True ($script:devices[0].Published -eq 'oem2.inf') 'Upgrade rollback did not select the previous NTPods package.'
    Assert-Fails { Restore-NativeAudioBindings $root $bindings @() $checkpoint } 'Rollback accepted a missing previous package.'
    $badPackages = @([pscustomobject]@{ Published = 'oem2.inf'; Name = 'NTPodsAudio.inf'; Inf = '..\outside.inf' })
    $before = $script:calls.Count
    Assert-Fails { Restore-NativeAudioBindings $root $bindings $badPackages $checkpoint } 'Rollback accepted a checkpoint path traversal.'
    Assert-True ($script:calls.Count -eq $before) 'Invalid checkpoint path reached a driver operation.'
    Write-Host 'PASS previous NTPods playback package rollback'

    Reset-Fixture 'failed-restore'
    Save-NativeAudioOriginals
    Install-NativeAudioDriver $root $audioInf
    $script:failBind = $true
    Assert-Fails { Uninstall-NativeAudioDriver $root } 'Injected binding failure did not fail restore.'
    Assert-True (Test-Path -LiteralPath (Join-Path (Get-NativeAudioBackupPath) 'bindings.json')) 'Failed restore deleted the original backup.'
    $firstBackup = @(Read-NativeAudioBackup)[0]
    Assert-True ((Get-NativeAudioRestoreInf $firstBackup) -like '*AlternativeA2DP.inf') 'Failed restore lost the saved driver.'
    $script:devices[0].Service = 'BthA2dp'; $script:devices[0].Published = 'microsoft_bluetooth_a2dp_src.inf'
    Uninstall-NativeAudioDriver $root
    Assert-True ($script:devices[0].Service -eq 'AltA2DP' -and $script:devices[1].Service -eq 'BthA2dp') 'Retry did not restore playback after the native package was removed.'
    Assert-True (-not (Test-Path -LiteralPath (Get-NativeAudioBackupPath))) 'Successful retry retained the backup.'
    Write-Host 'PASS failed restore retains the backup'

    Reset-Fixture 'absent-restore'
    Save-NativeAudioOriginals
    Install-NativeAudioDriver $root $audioInf
    $script:devices[2].Service = 'NTPodsAudio'; $script:devices[2].Published = 'oem1.inf'
    Uninstall-NativeAudioDriver $root
    Assert-True (Test-Path -LiteralPath (Get-NativeAudioBackupPath)) 'An absent device lost its pending original-provider restore.'
    Assert-True (-not @($script:calls | Where-Object { $_ -like ('bind:*|' + $script:devices[2].InstanceId + '|*') }).Count) 'Uninstall bound an absent device.'
    $script:devices[2].Present = $true
    $script:devices[2].Service = 'BthA2dp'; $script:devices[2].Published = 'microsoft_bluetooth_a2dp_src.inf'
    Uninstall-NativeAudioDriver $root
    Assert-True ($script:devices[2].Service -eq 'AltA2DP') 'Reconnected device did not receive its saved playback provider.'
    Assert-True (-not (Test-Path -LiteralPath (Get-NativeAudioBackupPath))) 'Completed absent-device restore kept its backup.'
    Write-Host 'PASS absent-device restore resumes after reconnect'

    foreach ($bad in @(
        [pscustomobject]@{ Published = 'oem90.inf'; Inf = '..\outside.inf' },
        [pscustomobject]@{ Published = 'oem90.inf'; Inf = 'driver.dll' },
        [pscustomobject]@{ Published = 'oem90.inf'; Inf = 'missing.inf' },
        [pscustomobject]@{ Published = 'oem90.inf'; Inf = $null }
    )) {
        Assert-Fails { Get-NativeAudioRestoreInf $bad } 'Malformed or stale restore path was accepted.'
    }
    foreach ($malformed in @('null', '42', '"text"', '{}', '[{"Published":"oem90.inf"}]',
        '[{"InstanceId":"ROOT\\OTHER\\test","Published":"microsoft_bluetooth_a2dp_src.inf","Inf":null}]',
        ('[{"InstanceId":' + (ConvertTo-Json "$pro1\first") + ',"Published":"other.inf","Inf":null}]'))) {
        Reset-Fixture ('malformed-' + [guid]::NewGuid())
        $path = Get-NativeAudioBackupPath
        New-Item -ItemType Directory -Path $path -Force | Out-Null
        Set-Content -LiteralPath (Join-Path $path 'bindings.json') -Value $malformed
        $script:devices[0].Service = 'NTPodsAudio'; $script:devices[0].Published = 'oem1.inf'
        Assert-Fails { Uninstall-NativeAudioDriver $root } 'Malformed original-driver manifest was accepted.'
        Assert-True ($script:calls.Count -eq 0) 'Malformed backup changed a driver before rejection.'
    }
    Reset-Fixture 'duplicate-backup'
    $entry = [pscustomobject]@{ InstanceId = "$pro1\first"; Published = 'microsoft_bluetooth_a2dp_src.inf'; Inf = $null }
    Write-Backup @($entry, $entry)
    Assert-Fails { Read-NativeAudioBackup } 'Duplicate device entries were accepted.'
    Assert-True ($script:calls.Count -eq 0) 'Duplicate entries reached a driver operation.'

    foreach ($badPath in '..\outside.inf', 'missing.inf') {
        Reset-Fixture ('stale-' + [guid]::NewGuid())
        Write-Backup @([pscustomobject]@{ InstanceId = "$pro1\first"; Published = 'oem90.inf'; Inf = $badPath })
        $script:devices[0].Service = 'NTPodsAudio'; $script:devices[0].Published = 'oem1.inf'
        Assert-Fails { Uninstall-NativeAudioDriver $root } 'An invalid original-driver path was accepted during uninstall.'
        Assert-True ($script:calls.Count -eq 0) 'Invalid original-driver path changed a driver before rejection.'
        Assert-True (Test-Path -LiteralPath (Join-Path (Get-NativeAudioBackupPath) 'bindings.json')) 'Rejected backup was deleted.'
    }
    Write-Host 'PASS invalid backup paths and manifests fail before driver changes'
} finally {
    Remove-Item -LiteralPath ('Function:\' + $helper) -ErrorAction SilentlyContinue
    $env:ProgramData = $oldProgramData
    if (-not [IO.Path]::GetFullPath($testRoot).StartsWith($tempRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe test cleanup path.' }
    if (Test-Path -LiteralPath $testRoot) { Remove-Item -LiteralPath $testRoot -Recurse -Force }
}
