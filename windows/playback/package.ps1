#requires -Version 7.0
[CmdletBinding()]
param(
    [string]$BuildOutput = (Join-Path $PSScriptRoot '..\build-output\native-playback'),
    [string]$Downloads = (Join-Path $PSScriptRoot '..\build-output\downloads')
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$BuildOutput = (Resolve-Path -LiteralPath $BuildOutput).Path
$manifest = Get-Content -LiteralPath (Join-Path $BuildOutput 'build-manifest.json') -Raw | ConvertFrom-Json
$commit = & git -C $repo rev-parse HEAD
if ($LASTEXITCODE) { throw 'Cannot identify the source commit.' }
if ($manifest.dirty -or $manifest.commit -ne $commit) { throw 'Playback downloads require a clean build of the current commit.' }
if (& git -C $repo status --porcelain) { throw 'Source changed after the build; commit and rebuild before packaging.' }
if ($LASTEXITCODE) { throw 'Cannot verify the source checkout.' }
if (@($manifest.codecs).Count -ne 1 -or $manifest.codecs[0] -ne 'aac') { throw 'Only native AAC builds may be packaged for download.' }
$required = @(
    'runtime/ntpodsd.exe', 'runtime/avcodec-61.dll', 'runtime/avutil-59.dll', 'runtime/swresample-5.dll',
    'runtime/LICENSE-ffmpeg.txt', 'runtime/THIRD_PARTY.md',
    'driver/NTPodsAAP.sys', 'driver/NTPodsAAP.inf', 'driver/ntpodsaap.cat',
    'driver-mic-acx/AudioCodec.sys', 'driver-mic-acx/AudioCodec.inf', 'driver-mic-acx/audiocodec.cat'
)
if (@($manifest.files).Count -ne $required.Count) { throw 'Unexpected playback build files.' }
foreach ($relative in $required) {
    $entry = @($manifest.files | Where-Object path -eq $relative)
    $path = Join-Path $BuildOutput $relative
    if ($entry.Count -ne 1 -or -not (Test-Path -LiteralPath $path -PathType Leaf) -or
        (Get-FileHash -LiteralPath $path).Hash -ne $entry[0].sha256) { throw "Build file changed or is missing: $relative" }
}
$bundle = Join-Path $BuildOutput ('package-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $bundle -ErrorAction Stop | Out-Null
foreach ($relative in $required) {
    $destination = Join-Path $bundle $relative
    New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $BuildOutput $relative) -Destination $destination
}
foreach ($file in 'README.md', 'TESTING.md', 'preflight.ps1', 'verify-bundle.ps1') {
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination $bundle
}
foreach ($file in 'setup-common.ps1', 'stop-ntpods.ps1') {
    Copy-Item -LiteralPath (Join-Path $repo "windows/installer/$file") -Destination $bundle
}
Copy-Item -LiteralPath (Join-Path $BuildOutput 'build-manifest.json') -Destination $bundle
Copy-Item -LiteralPath (Join-Path $repo 'THIRD_PARTY.md') -Destination $bundle
Copy-Item -LiteralPath (Join-Path $repo 'LICENSE') -Destination $bundle
Copy-Item -LiteralPath (Join-Path $repo 'licenses') -Destination $bundle -Recurse
New-Item -ItemType Directory -Path (Join-Path $bundle 'tools') | Out-Null
Copy-Item -LiteralPath (Join-Path $repo 'windows/installer/tools/devcon.exe') -Destination (Join-Path $bundle 'tools')
Get-ChildItem -LiteralPath $bundle -Recurse -File | Sort-Object FullName | ForEach-Object {
    "$((Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant())  $($_.FullName.Substring($bundle.Length + 1).Replace('\', '/'))"
} | Set-Content -LiteralPath (Join-Path $bundle 'SHA256SUMS.txt') -Encoding ASCII
& (Join-Path $bundle 'verify-bundle.ps1') -Bundle $bundle
New-Item -ItemType Directory -Path $Downloads -Force | Out-Null
$name = 'NTPods-Native-AAC.zip'
$zip = Join-Path $Downloads $name
Compress-Archive -Path (Join-Path $bundle '*') -DestinationPath $zip -Force
$checksumsPath = Join-Path $Downloads 'SHA256SUMS.txt'
$checksums = if (Test-Path -LiteralPath $checksumsPath) {
    @(Get-Content -LiteralPath $checksumsPath | Where-Object { $_ -notmatch '  NTPods-Native-AAC\.zip$' })
} else { @() }
@($checksums) + "$((Get-FileHash -LiteralPath $zip).Hash.ToLowerInvariant())  $name" |
    Set-Content -LiteralPath $checksumsPath -Encoding ASCII
Write-Host "Native AAC test download: $zip"
