<#
    Assemble a complete NTPods-dist folder from this repository.

    Copy installer scripts and current-source binaries into one complete folder.
    Local builds and CI use this script. Missing installer inputs fail the build.

    Installer scripts, helpers and devcon come from the repository. The daemon,
    WinUI app and both driver packages must be built from the current source:

        windows\daemon                 cargo build --release --target x86_64-pc-windows-gnu
                                       (or the msvc target WITH -C target-feature=+crt-static)
        windows\winui\NTPods.WinUI  MSBuild -t:Publish -p:Configuration=Release -p:Platform=x64

    The WinUI app needs Visual Studio's MSBuild. With plain `dotnet publish`,
    the WindowsAppSDK PRI step fails with
    "Microsoft.Build.Packaging.Pri.Tasks.dll ... could not be loaded". And its
    publish output omits ntpods-winui.pri, the app's resource index. This script
    restores it from the Build output so strings and icons are available.

    Usage:  .\make-dist.ps1 [-Out <path>] [-DaemonExe <exe>] [-WinUIDir <dir>]
#>
[CmdletBinding()]
param(
    # Defaults to <repo parent>\NTPods-dist.
    [string]$Out,
    # The built daemon. Defaults to the local GNU cross-build.
    [string]$DaemonExe,
    # The folder holding ntpods-winui.exe. Defaults to the local publish output;
    # CI passes its plain Build output instead.
    [string]$WinUIDir,
    # Generated from current source, with fresh catalogs. Never use committed prebuilts.
    [Parameter(Mandatory)][string]$DriverPackages
)

$ErrorActionPreference = 'Stop'
$installer = $PSScriptRoot
$win       = Split-Path -Parent $installer          # ...\windows
$repo      = Split-Path -Parent $win                # the repo root
$winuiOut  = Join-Path $win 'winui\NTPods.WinUI\bin\x64\Release\net10.0-windows10.0.19041.0\win-x64'
if (-not $Out)       { $Out       = Join-Path (Split-Path -Parent $repo) 'NTPods-dist' }
if (-not $DaemonExe) { $DaemonExe = Join-Path $win 'daemon\target\x86_64-pc-windows-gnu\release\ntpodsd.exe' }
if (-not $WinUIDir)  { $WinUIDir  = Join-Path $winuiOut 'publish' }
$resolve = { param($p) $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($p) }
$Out = & $resolve $Out; $DaemonExe = & $resolve $DaemonExe; $WinUIDir = & $resolve $WinUIDir
$DriverPackages = (Resolve-Path -LiteralPath $DriverPackages).Path

foreach ($p in @($DaemonExe, (Join-Path $WinUIDir 'ntpods-winui.exe'))) {
    if (-not (Test-Path $p)) { throw "Not built yet: $p`nSee the header of this script for the build commands." }
}

Write-Host "==> Assembling $Out"
if ((Test-Path -LiteralPath $Out) -and (Get-ChildItem -LiteralPath $Out -Force | Select-Object -First 1) -and
    -not (Test-Path -LiteralPath (Join-Path $Out '.ntpods-dist'))) {
    throw 'Refusing to replace a nonempty folder that was not created by make-dist.ps1.'
}
New-Item -ItemType Directory -Force -Path $Out | Out-Null
if ((Get-Item -LiteralPath $Out -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'The distribution directory cannot be a reparse point.' }
Set-Content -LiteralPath (Join-Path $Out '.ntpods-dist') -Value 'NTPods generated distribution' -Encoding ASCII
foreach ($sub in 'driver', 'driver-mic-acx', 'tools', 'winui', 'licenses') {
    $d = Join-Path $Out $sub
    if (-not [IO.Path]::GetFullPath($d).StartsWith($Out.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Output path escapes the distribution directory.' }
    if (Test-Path $d) { Remove-Item -LiteralPath $d -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $d | Out-Null
}
# Retired files an older dist may still hold (lp-mic-rename.exe → rename-mic.ps1).
foreach ($old in 'lp-mic-rename.exe') {
    $p = Join-Path $Out $old
    if (Test-Path $p) { Remove-Item $p -Force }
}

# ---- installer + helper scripts ---------------------------------------------
Copy-Item (Join-Path $installer 'install.ps1')        $Out -Force
Copy-Item (Join-Path $installer 'setup-common.ps1')   $Out -Force
Copy-Item (Join-Path $installer 'stop-ntpods.ps1')    $Out -Force
Copy-Item (Join-Path $installer 'msi-setup.ps1')      $Out -Force
Copy-Item (Join-Path $installer 'fix-driver.ps1')     $Out -Force
Copy-Item (Join-Path $installer 'rename-mic.ps1')     $Out -Force
Copy-Item (Join-Path $installer 'uninstall.ps1')      $Out -Force
Copy-Item (Join-Path $installer 'tools\*')            (Join-Path $Out 'tools') -Recurse -Force
Copy-Item (Join-Path $repo 'LICENSE'), (Join-Path $repo 'THIRD_PARTY.md'), (Join-Path $repo 'licenses\*') (Join-Path $Out 'licenses') -Force
Copy-Item (Join-Path $win 'daemon\vendor\ffmpeg\LICENSE-ffmpeg.txt') (Join-Path $Out 'licenses') -Force
Copy-Item (Join-Path $win 'playback\encoder\NOTICE-FDK.txt') (Join-Path $Out 'licenses') -Force
Copy-Item (Join-Path $win 'playback\encoder\LICENSE') (Join-Path $Out 'licenses\LICENSE-encoder.txt') -Force
Copy-Item (Join-Path $win 'playback\encoder\vendor\fdk-aac-cac04476081a23c870fed05c36228cd02c5e7c3d.zip') (Join-Path $Out 'licenses\FDK-AAC-source.zip') -Force
if ((Get-FileHash -LiteralPath (Join-Path $Out 'licenses\FDK-AAC-source.zip')).Hash -ne
    '1B572FCB5E2A81AA0447A92CA95BFE4979F3EDFC09918E0627B726BCD0165FF1') {
    throw 'The bundled FDK source does not match the pinned encoder source.'
}

# ---- driver packages generated by windows/build.ps1 -------------------------
Copy-Item (Join-Path $DriverPackages 'driver\*') (Join-Path $Out 'driver') -Force
# This distribution supports only the Windows 11 22H2+ ACX microphone.
Copy-Item (Join-Path $DriverPackages 'driver-mic-acx\*') (Join-Path $Out 'driver-mic-acx') -Force
Get-ChildItem (Join-Path $Out 'driver'), (Join-Path $Out 'driver-mic-acx') -Filter 'README.md' |
    Remove-Item -Force -ErrorAction SilentlyContinue

# ---- daemon + the FFmpeg runtime it links against ---------------------------
Copy-Item $DaemonExe $Out -Force
Copy-Item (Join-Path $win 'daemon\vendor\ffmpeg\bin\*.dll') $Out -Force
foreach ($file in 'ntpods-eld-encoder.exe', 'fdk-aac.dll') {
    Copy-Item -LiteralPath (Join-Path $win "playback\encoder\build\Release\$file") -Destination $Out -Force
}

# ---- WinUI app (unpackaged + self-contained: a whole folder) ----------------
Copy-Item (Join-Path $WinUIDir '*') (Join-Path $Out 'winui') -Recurse -Force
$pri = Join-Path $Out 'winui\ntpods-winui.pri'
if (-not (Test-Path $pri)) {
    # Publish output drops it; the Build output next to publish\ still has it.
    $src = Join-Path (Split-Path -Parent $WinUIDir) 'ntpods-winui.pri'
    if (Test-Path $src) { Copy-Item $src (Join-Path $Out 'winui') -Force }
}

# ---- completeness check ----------------------------------------------------
$required = @(
    'install.ps1', 'uninstall.ps1', 'setup-common.ps1', 'msi-setup.ps1', 'fix-driver.ps1', 'rename-mic.ps1', 'tools\devcon.exe',
    'licenses\LICENSE', 'licenses\THIRD_PARTY.md', 'licenses\Microsoft-Public-License.txt', 'licenses\LICENSE-ffmpeg.txt',
    'licenses\NOTICE-FDK.txt', 'licenses\LICENSE-encoder.txt', 'licenses\FDK-AAC-source.zip',
    'driver\NTPodsAAP.inf', 'driver\NTPodsAAP.sys', 'driver\ntpodsaap.cat',
    'driver-mic-acx\AudioCodec.inf', 'driver-mic-acx\AudioCodec.sys', 'driver-mic-acx\audiocodec.cat',
    'ntpodsd.exe', 'avcodec-61.dll', 'avutil-59.dll', 'swresample-5.dll',
    'ntpods-eld-encoder.exe', 'fdk-aac.dll',
    'winui\ntpods-winui.exe', 'winui\ntpods-winui.pri'
)
$missing = @($required | Where-Object { -not (Test-Path (Join-Path $Out $_)) })
if ($missing) { throw "Dist is incomplete, missing: $($missing -join ', ')" }

# Each driver catalog stores the flat SHA1/SHA256 of its INF. If the INF was
# edited (or line-ending-converted) after inf2cat ran, the package won't install.
foreach ($pkg in @(@('driver\NTPodsAAP.inf', 'driver\ntpodsaap.cat'),
                   @('driver-mic-acx\AudioCodec.inf', 'driver-mic-acx\audiocodec.cat'))) {
    $catHex = [BitConverter]::ToString([IO.File]::ReadAllBytes((Join-Path $Out $pkg[1]))).Replace('-', '')
    $inf = Join-Path $Out $pkg[0]
    $inCat = @('SHA1', 'SHA256') | Where-Object { $catHex.Contains((Get-FileHash $inf -Algorithm $_).Hash) }
    if (-not $inCat) {
        throw "$($pkg[0]) is not the INF $($pkg[1]) was generated for. Regenerate the catalog (inf2cat) or restore the matching INF."
    }
}

# These binaries must run without the Visual C++ redistributable.
foreach ($binary in 'ntpodsd.exe', 'ntpods-eld-encoder.exe', 'fdk-aac.dll') {
    $bytes = [IO.File]::ReadAllBytes((Join-Path $Out $binary))
    $text = [Text.Encoding]::ASCII.GetString($bytes)
    foreach ($dll in 'VCRUNTIME140.dll', 'VCRUNTIME140_1.dll', 'MSVCP140.dll') {
        if ($text.IndexOf($dll, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw "$binary imports $dll. Build the backend and encoder with the static C runtime."
        }
    }
}

$size = '{0:N0} MB' -f ((Get-ChildItem $Out -Recurse -File | Measure-Object Length -Sum).Sum / 1MB)
$count = (Get-ChildItem $Out -Recurse -File).Count
Write-Host "==> Done: $count files, $size" -ForegroundColor Green
Write-Host "    Install with (elevated):  cd '$Out'; powershell -ExecutionPolicy Bypass -File .\install.ps1" -ForegroundColor DarkGray
