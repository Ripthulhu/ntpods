# AudioSink replacement is limited to the models listed in NTPodsAudio.inf.
function Get-NativeAudioDevices {
    $ids = @(
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2002',
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&200A',
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&200E',
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&200F',
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2013',
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2014',
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2019',
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&201B',
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&201F',
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2024',
        'BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2027'
    )
    @(Get-PnpDevice -ErrorAction Stop | Where-Object {
        @($_.HardwareID | Where-Object { $_ -in $ids }).Count -gt 0
    })
}

function Get-NativeAudioBindings {
    @(foreach ($device in Get-NativeAudioDevices) {
        $inf = (Get-PnpDeviceProperty -InstanceId $device.InstanceId -KeyName 'DEVPKEY_Device_DriverInfPath' -ErrorAction Stop).Data
        $service = (Get-PnpDeviceProperty -InstanceId $device.InstanceId -KeyName 'DEVPKEY_Device_Service' -ErrorAction Stop).Data
        if ($inf -and $inf -notmatch '^(oem\d+|microsoft_bluetooth_a2dp_src)\.inf$') {
            throw 'Unexpected AudioSink driver package name.'
        }
        [pscustomobject]@{ InstanceId = $device.InstanceId; Published = $inf; Service = $service }
    })
}

function Get-NativeAudioBackupPath { Join-Path $env:ProgramData 'NTPods\audio-original' }

function Save-NativeAudioJson([string]$path, [object[]]$entries) {
    $temporary = $path + '.' + [guid]::NewGuid().ToString('N') + '.tmp'
    try {
        [IO.File]::WriteAllText($temporary, (ConvertTo-Json -InputObject @($entries) -Depth 4))
        if (Test-Path -LiteralPath $path) { [IO.File]::Replace($temporary, $path, [NullString]::Value) }
        else { [IO.File]::Move($temporary, $path) }
    } finally {
        if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force }
    }
}

function Assert-NativeAudioInstance([string]$instanceId) {
    if ($instanceId -notmatch '\ABTHENUM\\\{0000110b-0000-1000-8000-00805f9b34fb\}_VID&0001004C_PID&(2002|200A|200E|200F|2013|2014|2019|201B|201F|2024|2027)\\[^\\\x00-\x20]+\z') {
        throw 'Invalid AirPods playback instance.'
    }
}

function Read-NativeAudioBackup {
    $path = Get-NativeAudioBackupPath
    if (Test-Path -LiteralPath $path) {
        @(Get-InstallTree $path) | Out-Null
        $file = Join-Path $path 'bindings.json'
        if (Test-Path -LiteralPath $file) {
            $json = Get-Content -LiteralPath $file -Raw
            if (-not $json.TrimStart().StartsWith('[')) { throw 'Playback backup must contain an array.' }
            $parsed = ConvertFrom-Json -InputObject $json
            $entries = @($parsed)
            $seen = @{}
            foreach ($entry in $entries) {
                if (($entry.PSObject.Properties.Name | Sort-Object) -join ',' -cne 'Inf,InstanceId,Published') { throw 'Invalid playback backup record.' }
                Assert-NativeAudioInstance $entry.InstanceId
                if ($seen.ContainsKey($entry.InstanceId)) { throw 'Duplicate playback backup record.' }
                $seen[$entry.InstanceId] = $true
                if ($entry.Published -notmatch '\A(oem\d+|microsoft_bluetooth_a2dp_src)\.inf\z') { throw 'Invalid playback backup package.' }
                Get-NativeAudioRestoreInf $entry | Out-Null
            }
            $entries
        }
    }
}

function Save-NativeAudioOriginals {
    $path = Get-NativeAudioBackupPath
    $saved = @(Read-NativeAudioBackup)
    $devices = @(Get-NativeAudioBindings)
    foreach ($device in $devices) {
        if ($device.Service -eq 'NTPodsAudio' -or $device.InstanceId -in $saved.InstanceId) { continue }
        if (-not $device.Published) { continue }
        New-Item -ItemType Directory -Path $path -Force | Out-Null
        if ($device.Published -match '^oem\d+\.inf$') {
            $folder = Join-Path $path $device.Published
            if ((Test-Path -LiteralPath $folder) -and $device.Published -notin $saved.Published) {
                if (-not [IO.Path]::GetFullPath($folder).StartsWith([IO.Path]::GetFullPath($path) + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe playback backup path.' }
                @(Get-InstallTree $folder) | Out-Null
                Remove-Item -LiteralPath $folder -Recurse -Force
            }
            if (-not (Test-Path -LiteralPath $folder)) {
                New-Item -ItemType Directory -Path $folder | Out-Null
                Invoke-Tool 'Backing up the previous playback driver' @(0) { pnputil /export-driver $device.Published $folder }
            }
            $infs = @(Get-ChildItem -LiteralPath $folder -Filter '*.inf' -Recurse -File)
            if ($infs.Count -ne 1) { throw 'The previous playback driver backup is incomplete.' }
            $inf = $infs[0].FullName.Substring($path.Length + 1)
        } else {
            $inf = $null
        }
        $saved += [pscustomobject]@{ InstanceId = $device.InstanceId; Published = $device.Published; Inf = $inf }
        # Save each entry before any new profile driver can bind.
        Save-NativeAudioJson (Join-Path $path 'bindings.json') $saved
    }
}

function Get-NativeAudioRestoreInf($binding) {
    if ($binding.Inf) {
        $base = [IO.Path]::GetFullPath((Get-NativeAudioBackupPath))
        if ([IO.Path]::IsPathRooted($binding.Inf)) { throw 'The playback backup path must be relative.' }
        $inf = [IO.Path]::GetFullPath((Join-Path $base $binding.Inf))
        if (-not $inf.StartsWith($base + '\', [StringComparison]::OrdinalIgnoreCase) -or
            [IO.Path]::GetExtension($inf) -ine '.inf') { throw 'Invalid playback driver backup path.' }
        if (-not (Test-Path -LiteralPath $inf -PathType Leaf)) { throw 'The previous playback driver backup is missing.' }
        return $inf
    }
    if ($binding.Published -and $binding.Published -ne 'microsoft_bluetooth_a2dp_src.inf') {
        throw 'The previous playback driver has no backup.'
    }
    $inf = Join-Path $env:SystemRoot 'INF\microsoft_bluetooth_a2dp_src.inf'
    if (-not (Test-Path -LiteralPath $inf -PathType Leaf)) { throw 'The Windows playback driver is missing.' }
    $inf
}

function Set-NativeAudioBinding([string]$root, [string]$instanceId, [string]$inf) {
    Assert-NativeAudioInstance $instanceId
    $helper = Join-Path $root 'tools\audio-binding.exe'
    if (-not (Test-Path -LiteralPath $helper)) { throw 'The playback driver installer is missing.' }
    Invoke-Tool 'Binding the AirPods playback driver' @(0, 3010) { & $helper --bind $instanceId $inf }
}

function Install-NativeAudioDriver([string]$root, [string]$inf) {
    $present = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | ForEach-Object InstanceId)
    foreach ($device in Get-NativeAudioDevices) {
        if ($device.InstanceId -notin $present) { continue }
        Set-NativeAudioBinding $root $device.InstanceId $inf
        $service = (Get-PnpDeviceProperty -InstanceId $device.InstanceId -KeyName 'DEVPKEY_Device_Service' -ErrorAction Stop).Data
        $problem = (Get-PnpDeviceProperty -InstanceId $device.InstanceId -KeyName 'DEVPKEY_Device_ProblemCode' -ErrorAction Stop).Data
        if ($service -ne 'NTPodsAudio' -or $null -eq $problem -or $problem -notin 0, 14) {
            throw 'The AirPods playback driver did not start. Restoring the previous drivers.'
        }
        if ($problem -eq 14) { Write-Host '==> AirPods playback requires a Windows restart.' }
    }
}

function Restore-NativeAudioBindings([string]$root, $bindings, $packages = @(), [string]$checkpoint = '') {
    $present = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | ForEach-Object InstanceId)
    foreach ($device in @($bindings)) {
        if ($device.InstanceId -notin $present) { continue }
        if ($device.Service -eq 'NTPodsAudio') {
            $package = @($packages | Where-Object { $_.Published -eq $device.Published -and $_.Name -eq 'NTPodsAudio.inf' })
            if ($package.Count -ne 1) { throw 'The previous NTPods playback package is missing.' }
            $base = [IO.Path]::GetFullPath($checkpoint)
            $inf = [IO.Path]::GetFullPath((Join-Path $base $package[0].Inf))
            if (-not $inf.StartsWith($base + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid playback rollback path.' }
        } else {
            $saved = @(Read-NativeAudioBackup | Where-Object InstanceId -eq $device.InstanceId)
            $inf = if ($saved.Count -eq 1) { Get-NativeAudioRestoreInf $saved[0] } else { Get-NativeAudioRestoreInf $null }
            Invoke-Tool 'Staging the previous playback driver' @(0, 259, 3010) { pnputil /add-driver $inf }
        }
        Set-NativeAudioBinding $root $device.InstanceId $inf
    }
}

function Uninstall-NativeAudioDriver([string]$root) {
    $savedBindings = @(Read-NativeAudioBackup)
    $path = Get-NativeAudioBackupPath
    $pendingPath = Join-Path $path 'restore-pending.json'
    $targets = @((Get-NativeAudioBindings | Where-Object Service -eq 'NTPodsAudio').InstanceId)
    if (Test-Path -LiteralPath $pendingPath) {
        $json = Get-Content -LiteralPath $pendingPath -Raw
        if (-not $json.TrimStart().StartsWith('[')) { throw 'Invalid playback recovery list.' }
        $pending = ConvertFrom-Json -InputObject $json
        foreach ($instance in @($pending)) { Assert-NativeAudioInstance $instance }
        $targets += @($pending)
    }
    $bindings = @($targets | Where-Object { $_ } | Select-Object -Unique | ForEach-Object {
        [pscustomobject]@{ InstanceId = $_; Published = ''; Service = '' }
    })
    $restoreInfs = @(foreach ($binding in $bindings) {
        $saved = @($savedBindings | Where-Object InstanceId -eq $binding.InstanceId)
        if ($saved.Count -eq 1) { Get-NativeAudioRestoreInf $saved[0] } else { Get-NativeAudioRestoreInf $null }
    }) + @(foreach ($saved in $savedBindings) { Get-NativeAudioRestoreInf $saved })
    # Validate and stage every fallback before removing a working playback driver.
    foreach ($inf in $restoreInfs | Select-Object -Unique) {
        Invoke-Tool 'Staging the previous playback package' @(0, 259, 3010) { pnputil /add-driver $inf }
    }
    if ($bindings.Count) {
        New-Item -ItemType Directory -Path $path -Force | Out-Null
        Save-NativeAudioJson $pendingPath @($bindings.InstanceId)
    }
    foreach ($oem in Get-DriverPackages 'NTPodsAudio.inf' @('NTPods')) {
        Invoke-Tool 'Removing NTPods playback' @(0, 3010) { pnputil /delete-driver $oem /uninstall /force }
    }
    Restore-NativeAudioBindings $root $bindings
    $present = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | ForEach-Object InstanceId)
    $absent = @($bindings.InstanceId | Where-Object { $_ -notin $present })
    if ($absent.Count) {
        Save-NativeAudioJson $pendingPath $absent
        Write-Warning "The previous playback packages are staged. Reconnect absent AirPods to finish driver selection. Binding backups remain at $path."
        return
    }
    $path = [IO.Path]::GetFullPath((Get-NativeAudioBackupPath))
    if ($path -ine [IO.Path]::GetFullPath((Join-Path $env:ProgramData 'NTPods\audio-original'))) { throw 'Unsafe playback backup cleanup path.' }
    if (Test-Path -LiteralPath $path) {
        @(Get-InstallTree $path) | Out-Null
        Remove-Item -LiteralPath $path -Recurse -Force
    }
}
