$ErrorActionPreference = 'Stop'
$sourceRepo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('ntpods-release-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path (Join-Path $fixture 'dist') -Force | Out-Null
try {
    $repo = Join-Path $fixture 'source'
    New-Item -ItemType Directory -Path (Join-Path $repo 'windows'), (Join-Path $repo 'docs') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $sourceRepo 'windows/package-release.ps1') -Destination (Join-Path $repo 'windows')
    Copy-Item -LiteralPath (Join-Path $sourceRepo 'docs/WINDOWS-RELEASE-NOTES.md') -Destination (Join-Path $repo 'docs')
    & git -C $repo init --quiet
    & git -C $repo add .
    & git -C $repo -c user.name=NTPods-test -c user.email=test@example.invalid -c commit.gpgsign=false commit --quiet -m fixture
    if ($LASTEXITCODE) { throw 'Failed to prepare the source fixture.' }
    $manifestPath = Join-Path $fixture 'build-manifest.json'
    $payload = Join-Path $fixture 'dist/test.txt'
    Set-Content -LiteralPath $payload -Value 'fixture'
    Set-Content -LiteralPath (Join-Path $fixture 'NTPods-experimental.msi') -Value 'fixture, not an installer'
    $msi = Join-Path $fixture 'NTPods-experimental.msi'
    $manifest = @{ commit = (& git -C $repo rev-parse HEAD); dirty = $false; msi = @{path='NTPods-experimental.msi';sha256=(Get-FileHash -LiteralPath $msi).Hash}; files = @(@{ path = 'dist/test.txt'; sha256 = (Get-FileHash -LiteralPath $payload).Hash }) }
    function Write-Manifest { $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath }
    function Reject([string]$expected) {
        try { & (Join-Path $repo 'windows/package-release.ps1') -BuildOutput $fixture }
        catch { if ($_.Exception.Message -like $expected) { return }; throw }
        throw "Expected rejection: $expected"
    }
    $manifest.dirty = $true
    Write-Manifest
    Reject '*clean build*'
    $manifest.dirty = $false
    $manifest.commit = '0000000000000000000000000000000000000000'
    Write-Manifest
    Reject '*current commit*'
    $manifest.commit = & git -C $repo rev-parse HEAD
    Write-Manifest
    Set-Content -LiteralPath (Join-Path $repo 'after-build.txt') -Value 'changed after build'
    Reject '*Source changed after the build*'
    Remove-Item -LiteralPath (Join-Path $repo 'after-build.txt')
    $manifest.msi.sha256 = 'BAD'
    Write-Manifest
    Reject '*MSI differs*'
    $manifest.msi.sha256 = (Get-FileHash -LiteralPath $msi).Hash
    $manifest.files[0].sha256 = 'BAD'
    Write-Manifest
    Reject '*changed or is missing*'
    $manifest.files[0].path = '../escape.txt'
    Write-Manifest
    Reject '*leaves the distribution*'
    $manifest.files[0].path = 'dist/test.txt'
    $manifest.files[0].sha256 = (Get-FileHash -LiteralPath $payload).Hash
    Write-Manifest
    Set-Content -LiteralPath (Join-Path $fixture 'dist/extra.txt') -Value 'untracked'
    Reject '*absent from the manifest*'
    Remove-Item -LiteralPath (Join-Path $fixture 'dist/extra.txt')
    & (Join-Path $repo 'windows/package-release.ps1') -BuildOutput $fixture
    $downloads = Join-Path $fixture 'downloads'
    foreach ($line in Get-Content -LiteralPath (Join-Path $downloads 'SHA256SUMS.txt')) {
        $hash, $name = $line -split '  ', 2
        if ((Get-FileHash -LiteralPath (Join-Path $downloads $name)).Hash -ne $hash) { throw "Wrong asset checksum: $name" }
    }
    $zip = [IO.Compression.ZipFile]::OpenRead((Join-Path $downloads 'NTPods-Windows.zip'))
    try {
        if (-not $zip.GetEntry('test.txt') -or -not $zip.GetEntry('READ-BEFORE-INSTALLING.md')) { throw 'ZIP is missing payload or warning.' }
    } finally { $zip.Dispose() }
    Write-Host 'Release package: malformed/stale inputs rejected; ZIP warning, source and asset checksums passed.'
} finally {
    $resolved = [IO.Path]::GetFullPath($fixture)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path -Leaf $resolved) -notlike 'ntpods-release-test-*') { throw 'Unsafe test cleanup path.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
