# Keep the pinned FFmpeg source and build recipes beside the binary downloads.
[CmdletBinding()]
param([string]$Downloads = (Join-Path $PSScriptRoot 'build-output/downloads'))
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
New-Item -ItemType Directory -Path $Downloads -Force | Out-Null
$Downloads = (Resolve-Path -LiteralPath $Downloads).Path
$sources = @(
    @{
        Name = 'FFmpeg-source.zip'
        Url = 'https://codeload.github.com/FFmpeg/FFmpeg/zip/1fdbca85aaea513c9cc6c14d347f76543346d3da'
        Sha256 = 'c0287c1dd38de7fdd3e9de8daacaa2ea679faf3973007795a1b5310972f5205e'
    },
    @{
        Name = 'FFmpeg-build-recipes.zip'
        Url = 'https://codeload.github.com/BtbN/FFmpeg-Builds/zip/a99e8230eae00d1cee38f23076a7a1f55cd984e2'
        Sha256 = '32bd407c060e53737d5db008e75f52e5b8d63adfe98490be3c044fa5e3628ccd'
    }
)
foreach ($source in $sources) {
    $path = Join-Path $Downloads $source.Name
    if (Test-Path -LiteralPath $path) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or (Get-FileHash -LiteralPath $path).Hash -ne $source.Sha256) {
            throw "Cached source archive failed verification: $($source.Name)"
        }
        continue
    }
    $temporary = Join-Path $Downloads ([Guid]::NewGuid().ToString() + '.tmp')
    try {
        Invoke-WebRequest -Uri $source.Url -OutFile $temporary -UseBasicParsing
        if ((Get-FileHash -LiteralPath $temporary).Hash -ne $source.Sha256) {
            throw "Downloaded source archive failed verification: $($source.Name)"
        }
        Move-Item -LiteralPath $temporary -Destination $path
    } finally {
        if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force }
    }
}
$notesName = 'FFmpeg-SOURCE-NOTES.md'
Copy-Item -LiteralPath (Join-Path (Split-Path -Parent $PSScriptRoot) 'docs/FFMPEG-SOURCES.md') -Destination (Join-Path $Downloads $notesName) -Force
$assets = @($sources | ForEach-Object { $_.Name }) + $notesName
$checksumsPath = Join-Path $Downloads 'SHA256SUMS.txt'
$checksums = @()
if (Test-Path -LiteralPath $checksumsPath) {
    $checksums = @(Get-Content -LiteralPath $checksumsPath | Where-Object {
        -not ($_ -match '^\S+\s+\*?(.+)$' -and $Matches[1] -in $assets)
    })
}
$checksums += @($assets | ForEach-Object {
    "$((Get-FileHash -LiteralPath (Join-Path $Downloads $_)).Hash.ToLowerInvariant())  $_"
})
$temporary = Join-Path $Downloads ([Guid]::NewGuid().ToString() + '.tmp')
try {
    $checksums | Set-Content -LiteralPath $temporary -Encoding ASCII
    Move-Item -LiteralPath $temporary -Destination $checksumsPath -Force
} finally {
    if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force }
}
Write-Host "Verified FFmpeg sources and build recipes in $Downloads"
