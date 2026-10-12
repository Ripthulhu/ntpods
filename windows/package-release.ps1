# Package only the tested build and its exact committed source.
[CmdletBinding()]
param([string]$BuildOutput = (Join-Path $PSScriptRoot 'build-output'))
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$BuildOutput = (Resolve-Path -LiteralPath $BuildOutput).Path
$manifestPath = Join-Path $BuildOutput 'build-manifest.json'
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$commit = & git -C $repo rev-parse HEAD
if ($LASTEXITCODE) { throw 'Cannot identify the source commit.' }
if ($manifest.dirty -or $manifest.commit -ne $commit) { throw 'Downloads require a clean build of the current commit.' }
if (& git -C $repo status --porcelain) { throw 'Source changed after the build; commit and rebuild before packaging.' }
if ($LASTEXITCODE) { throw 'Cannot verify the source checkout.' }
$msi = Join-Path $BuildOutput 'NTPods-experimental.msi'
if (-not (Test-Path -LiteralPath $msi -PathType Leaf)) { throw 'The tested MSI is missing.' }
if ($manifest.msi.path -ne 'NTPods-experimental.msi' -or (Get-FileHash -LiteralPath $msi).Hash -ne $manifest.msi.sha256) { throw 'MSI differs from the tested build.' }
$dist = Join-Path $BuildOutput 'dist'
if (-not $manifest.files.Count) { throw 'The build manifest contains no distribution files.' }
foreach ($file in $manifest.files) {
    $path = [IO.Path]::GetFullPath((Join-Path $BuildOutput $file.path))
    if (-not $path.StartsWith($dist + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Manifest path leaves the distribution directory.' }
    if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or (Get-FileHash -LiteralPath $path).Hash -ne $file.sha256) { throw "Build file changed or is missing: $($file.path)" }
}
if (@(Get-ChildItem -LiteralPath $dist -Recurse -File -Force).Count -ne $manifest.files.Count) { throw 'The distribution contains files absent from the manifest.' }
$downloads = Join-Path $BuildOutput 'downloads'
New-Item -ItemType Directory -Path $downloads -Force | Out-Null
Copy-Item -LiteralPath $msi -Destination (Join-Path $downloads 'NTPods.msi') -Force
Copy-Item -LiteralPath $manifestPath -Destination $downloads -Force
$notes = Join-Path $downloads 'RELEASE-NOTES.md'
Copy-Item -LiteralPath (Join-Path $repo 'docs/WINDOWS-RELEASE-NOTES.md') -Destination $notes -Force
$text = (Get-Content -LiteralPath $notes -Raw).Replace('/blob/codex/windows-airpods-hardening/', "/blob/$commit/")
Set-Content -LiteralPath $notes -Value $text -Encoding UTF8
Add-Content -LiteralPath $notes -Value "`nSource commit: $commit"
if ($env:GITHUB_SERVER_URL -and $env:GITHUB_REPOSITORY -and $env:GITHUB_RUN_ID) {
    Add-Content -LiteralPath $notes -Value "Build: $env:GITHUB_SERVER_URL/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID"
}
$zip = Join-Path $downloads 'NTPods-Windows.zip'
Compress-Archive -Path (Join-Path $dist '*') -DestinationPath $zip -Force
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::Open($zip, [IO.Compression.ZipArchiveMode]::Update)
try { [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $notes, 'READ-BEFORE-INSTALLING.md') | Out-Null }
finally { $archive.Dispose() }
& git -C $repo archive --format=zip "--output=$(Join-Path $downloads 'NTPods-source.zip')" $commit
if ($LASTEXITCODE) { throw 'Source archive creation failed.' }
$assets = 'NTPods.msi', 'NTPods-Windows.zip', 'NTPods-source.zip', 'build-manifest.json', 'RELEASE-NOTES.md'
$assets | ForEach-Object { "$((Get-FileHash -LiteralPath (Join-Path $downloads $_)).Hash.ToLowerInvariant())  $_" } |
    Set-Content -LiteralPath (Join-Path $downloads 'SHA256SUMS.txt') -Encoding ASCII
Write-Host "Prepared experimental downloads in $downloads"
