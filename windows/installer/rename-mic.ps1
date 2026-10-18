<#
    Rename the ACX microphone to match the connected device without restarting
    audio. Requires administrator access to the endpoint registry properties.

    The installed helper reads the name with the user's unelevated permissions.
    To supply a name manually, run elevated: .\rename-mic.ps1 -Name "AirPods Pro"
#>
param([Parameter(Mandatory=$true)][string]$Name)
$ErrorActionPreference = 'Stop'

function Log([string]$msg) {
    $line = '{0:yyyy-MM-ddTHH:mm:ss.fffZ} {1}' -f (Get-Date).ToUniversalTime(), $msg
    Write-Host $line
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

# Return failure to the scheduled helper.
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
        # Administrators can set these values but lack full KEY_WRITE rights;
        # OpenSubKey(..., $true) would fail even when elevated.
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
