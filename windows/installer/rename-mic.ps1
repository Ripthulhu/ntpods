<#
    Show the ACX microphone under the connected device's name in Windows apps.
    Requires administrator access to the endpoint registry properties.

    Two ways in:
      * The "NTPods Rename Mic" scheduled task (RunLevel Highest), which
        install.ps1 registers. The daemon writes the name to
        %LOCALAPPDATA%\NTPods\micname.txt and fires the task with
        `schtasks /run` (daemon/src/rename.rs). No -Name, runs hidden.
      * By hand, elevated:   .\rename-mic.ps1 "AirPods Pro"

    Match ROOT\NTPodsMicACX and update PKEY_Device_DeviceDesc only when the name
    changes. Renaming does not restart the audio service.
#>
param([string]$Name)
$ErrorActionPreference = 'Stop'

$dir = Join-Path $env:LOCALAPPDATA 'NTPods'
function Log([string]$msg) {
    $line = '{0:yyyy-MM-ddTHH:mm:ss.fffZ} {1}' -f (Get-Date).ToUniversalTime(), $msg  # UTC, like daemon.log
    Write-Host $line
}

if (-not $Name) {
    $file = Join-Path $dir 'micname.txt'
    if (-not (Test-Path $file)) { Log "no -Name given and $file does not exist"; exit 1 }
    if ((Get-Item -LiteralPath $file).Length -gt 1KB) { throw 'The microphone name file is too large.' }
    $Name = (Get-Content $file -Raw -Encoding UTF8).Trim()
}
if (-not $Name) { Log 'empty name, nothing to do'; exit 1 }
if ($Name.Length -gt 128 -or $Name -match '[\x00-\x1F\x7F]') { throw 'Invalid microphone name.' }

$base    = 'SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture'
$hwKey   = '{a8b865dd-2e3d-4094-ad97-e593a70c75d6},8'   # hardware id of the backing device
$ifKey   = '{b3f8fa53-0004-438e-9003-51a46e139bfc},6'   # PKEY_DeviceInterface_FriendlyName
$descKey = '{a45c254e-df1c-4efd-8020-67d146a850e0},2'   # PKEY_Device_DeviceDesc

# Values are normally REG_SZ; tolerate the REG_BINARY PROPVARIANT form (4-byte
# type tag + UTF-16) that some endpoints use.
function Read-Str($key, [string]$valueName) {
    $v = $key.GetValue($valueName)
    if ($v -is [byte[]]) {
        if ($v.Length -le 4) { return '' }
        return [Text.Encoding]::Unicode.GetString($v, 4, $v.Length - 4).TrimEnd([char]0)
    }
    if ($null -eq $v) { return '' }
    return [string]$v
}

# Report errors to the helper's captured output.
trap { Log "failed: $_"; exit 1 }

$root = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($base)
if (-not $root) { Log "cannot open HKLM\$base"; exit 1 }
$matched = 0; $changed = 0
try {
    foreach ($id in $root.GetSubKeyNames()) {
        $props = $root.OpenSubKey("$id\Properties")
        if (-not $props) { continue }
        try {
            $hw    = Read-Str $props $hwKey
            $iface = Read-Str $props $ifKey
            $desc  = Read-Str $props $descKey
        } finally { $props.Close() }
        if ($hw -ne 'ROOT\NTPodsMicACX') { continue }

        $matched++
        if ($desc -ceq $Name) { Log "$id already '$Name'"; continue }
        # The key belongs to Audiosrv/TrustedInstaller; Administrators only get
        # SetValue on it, so OpenSubKey(..., $true) (full KEY_WRITE) is denied even
        # elevated. Ask for exactly what the ACL grants.
        $w = $root.OpenSubKey("$id\Properties",
            [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree,
            [Security.AccessControl.RegistryRights]::QueryValues -bor [Security.AccessControl.RegistryRights]::SetValue)
        try { $w.SetValue($descKey, $Name, [Microsoft.Win32.RegistryValueKind]::String) } finally { $w.Close() }
        Log "$id '$desc' -> '$Name'"
        $changed++
    }
} finally { $root.Close() }

if (-not $matched) { Log 'no NTPods capture endpoint found - is the NTPodsMic driver installed?'; exit 1 }
if ($changed) {
    Log 'Name updated; applications will pick it up when the endpoint reopens.'
}
