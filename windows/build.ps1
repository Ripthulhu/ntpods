# Build the portable NTPods USB dongle preview.
[CmdletBinding()]
param([string]$Out = (Join-Path $PSScriptRoot 'build-output'))

$ErrorActionPreference = 'Stop'
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$msbuild = & $vswhere -latest -products '*' -version '[18.0,19.0)' -requires Microsoft.Component.MSBuild -find 'MSBuild\Current\Bin\MSBuild.exe' | Select-Object -First 1
if (-not $msbuild) { throw 'Install Visual Studio 2026 Build Tools with the WinUI build workload.' }
$vc = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1
if (-not $vc) { throw 'Install the Visual C++ x64 build tools and Windows SDK.' }
Import-Module (Join-Path $vc 'Common7/Tools/Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $vc -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$Out = [IO.Path]::GetFullPath($Out)
$package = Join-Path $Out "NTPods-Dongle-$([guid]::NewGuid().ToString('N').Substring(0,8))"
New-Item -ItemType Directory -Path $package | Out-Null

Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    & python.exe tools/check_dongle_protocol.py
    if ($LASTEXITCODE) { throw 'Dongle protocol check failed.' }
    & python.exe windows/tests/test-localization.py
    if ($LASTEXITCODE) { throw 'Localization check failed.' }
    & $cargo test --locked --manifest-path windows/daemon/Cargo.toml
    if ($LASTEXITCODE) { throw 'Daemon tests failed.' }
    & pwsh -NoProfile -File windows/tests/test-ipc.ps1
    if ($LASTEXITCODE) { throw 'IPC checks failed.' }
    & dotnet run --project windows/tests/DeviceSettings --configuration Release
    if ($LASTEXITCODE) { throw 'Device controls checks failed.' }
    & $cargo build --locked --release --manifest-path windows/daemon/Cargo.toml
    if ($LASTEXITCODE) { throw 'Daemon build failed.' }
    & $msbuild windows/winui/NTPods.WinUI/NTPods.WinUI.csproj -restore -t:Build -p:Configuration=Release -p:Platform=x64 -p:VisualStudioVersion=18.0 -p:RestorePackagesWithLockFile=true -p:RestoreLockedMode=true -nr:false -v:minimal "-p:OutDir=$package/"
    if ($LASTEXITCODE) { throw 'WinUI build failed.' }
    Copy-Item -LiteralPath windows/daemon/target/release/ntpodsd.exe -Destination (Join-Path $package 'ntpodsd-dongle.exe')
    Copy-Item -LiteralPath LICENSE, THIRD_PARTY.md -Destination $package
    Copy-Item -LiteralPath licenses -Destination $package -Recurse
    if (-not (Test-Path -LiteralPath (Join-Path $package 'ntpods-winui.exe'))) { throw 'Missing WinUI executable.' }
    foreach ($sceneFile in 'index.html', 'room.css', 'geometry.js', 'studio.js', 'room.js') {
        if (-not (Test-Path -LiteralPath (Join-Path $package "Assets/SpatialRoom/$sceneFile"))) {
            throw "Missing listening room asset: $sceneFile"
        }
    }
    $spatial = Join-Path $PSScriptRoot 'spatial'
    $manifest = Join-Path $spatial 'Cargo.toml'
    $target = Join-Path $spatial 'target'
    $cargoArgs = @('--locked', '--release', '--manifest-path', $manifest, '--target-dir', $target)
    $messages = & $cargo build @cargoArgs --message-format=json-render-diagnostics
    if ($LASTEXITCODE) { throw 'Spatial renderer build failed.' }
    $sdk = $messages | ConvertFrom-Json | Where-Object {
        $_.reason -eq 'build-script-executed' -and $_.package_id -match '(?:#|/)audionimbus-sys@'
    }
    if (@($sdk).Count -ne 1) { throw 'Cargo did not identify a unique Steam Audio runtime.' }
    $runtime = Join-Path $sdk.out_dir 'lib'
    $destination = New-Item -ItemType Directory -Path (Join-Path $package 'spatial')
    foreach ($file in 'front-voice.wav', 'HRTF-CREDITS.md', 'LICENSE-Apache-2.0.txt') {
        Copy-Item -LiteralPath (Join-Path $spatial $file) -Destination $destination.FullName
    }
    $profiles = Join-Path $target 'ear-profiles'
    & python.exe (Join-Path $spatial 'prepare-hrtf-profiles.py') --output $profiles
    if ($LASTEXITCODE) { throw 'Ear-profile verification failed.' }
    Copy-Item -LiteralPath (Join-Path $profiles 'H3.sofa'), (Join-Path $profiles 'H20.sofa') -Destination $destination.FullName
    Copy-Item -LiteralPath (Join-Path $runtime 'phonon.dll') -Destination $destination.FullName
    Copy-Item -LiteralPath (Join-Path $sdk.out_dir 'steam_audio_cache/steamaudio_core/steamaudio/THIRDPARTY.md') -Destination (Join-Path $destination 'STEAM-AUDIO-NOTICES.md')
    Copy-Item -LiteralPath (Join-Path $target 'release/windows-surround.exe') -Destination $destination.FullName
    $oldPath = $env:PATH
    try {
        $env:PATH = "$runtime;$oldPath"
        & $cargo test @cargoArgs
        if ($LASTEXITCODE) { throw 'Spatial renderer tests failed.' }
        & $cargo test @cargoArgs streaming_7_1_across_independent_clocks -- --ignored
        if ($LASTEXITCODE) { throw 'Spatial surround stability test failed.' }
    } finally {
        $env:PATH = $oldPath
    }
    & (Join-Path $destination 'windows-surround.exe') self-test
    if ($LASTEXITCODE) { throw 'Packaged spatial renderer failed its self-test.' }
    Compress-Archive -Path (Join-Path $package '*') -DestinationPath "$package.zip"
    Write-Host "Portable preview: $package.zip"
} finally {
    Pop-Location
}
