$ErrorActionPreference = 'Stop'
$sourceRepo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('ntpods-playback-package-' + [guid]::NewGuid().ToString('N'))
$repo = Join-Path $fixture 'source'
$built = Join-Path $fixture 'build'
$downloads = Join-Path $fixture 'downloads'
New-Item -ItemType Directory -Path (Join-Path $repo 'windows/playback'),
    (Join-Path $repo 'windows/installer/tools'), (Join-Path $repo 'licenses'), $built -Force | Out-Null
try {
    foreach ($file in 'package.ps1', 'verify-bundle.ps1', 'preflight.ps1', 'README.md', 'TESTING.md') {
        Copy-Item -LiteralPath (Join-Path $sourceRepo "windows/playback/$file") -Destination (Join-Path $repo 'windows/playback')
    }
    foreach ($file in 'setup-common.ps1', 'stop-ntpods.ps1', 'tools/devcon.exe') {
        Set-Content -LiteralPath (Join-Path $repo "windows/installer/$file") -Value 'fixture'
    }
    foreach ($file in 'LICENSE', 'THIRD_PARTY.md', 'licenses/notice.txt') {
        Set-Content -LiteralPath (Join-Path $repo $file) -Value 'fixture'
    }
    & git -C $repo init --quiet
    & git -C $repo add .
    & git -C $repo -c user.name=NTPods-test -c user.email=test@example.invalid -c commit.gpgsign=false commit --quiet -m fixture
    if ($LASTEXITCODE) { throw 'Failed to prepare source fixture.' }
    $files = @(
        'runtime/ntpodsd.exe', 'runtime/avcodec-61.dll', 'runtime/avutil-59.dll', 'runtime/swresample-5.dll',
        'runtime/LICENSE-ffmpeg.txt', 'runtime/THIRD_PARTY.md',
        'driver/NTPodsAAP.sys', 'driver/NTPodsAAP.inf', 'driver/ntpodsaap.cat',
        'driver-mic-acx/AudioCodec.sys', 'driver-mic-acx/AudioCodec.inf', 'driver-mic-acx/audiocodec.cat'
    )
    $manifest = @{ commit = (& git -C $repo rev-parse HEAD); dirty = $false; codecs = @('aac'); files = @() }
    foreach ($relative in $files) {
        $path = Join-Path $built $relative
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        Set-Content -LiteralPath $path -Value "fixture: $relative"
        $manifest.files += @{ path = $relative; sha256 = (Get-FileHash -LiteralPath $path).Hash }
    }
    function Write-Manifest { $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $built 'build-manifest.json') }
    function Package { & (Join-Path $repo 'windows/playback/package.ps1') -BuildOutput $built -Downloads $downloads }
    function Reject([scriptblock]$action, [string]$expected) {
        try { & $action }
        catch { if ($_.Exception.Message -like $expected) { return }; throw }
        throw "Expected rejection: $expected"
    }
    $manifest.dirty = $true
    Write-Manifest
    Reject { Package } '*clean build*'
    $manifest.dirty = $false
    $commit = $manifest.commit
    $manifest.commit = '0000000000000000000000000000000000000000'
    Write-Manifest
    Reject { Package } '*current commit*'
    $manifest.commit = $commit
    $manifest.codecs = @('aac', 'eld')
    Write-Manifest
    Reject { Package } '*Only native AAC*'
    $manifest.codecs = @('aac')
    $hash = $manifest.files[0].sha256
    $manifest.files[0].sha256 = 'BAD'
    Write-Manifest
    Reject { Package } '*changed or is missing*'
    $manifest.files[0].sha256 = $hash
    $manifest.files[0].path = '../escape.exe'
    Write-Manifest
    Reject { Package } '*changed or is missing*'
    $manifest.files[0].path = $files[0]
    Write-Manifest
    Set-Content -LiteralPath (Join-Path $repo 'changed.txt') -Value 'changed'
    Reject { Package } '*Source changed*'
    Remove-Item -LiteralPath (Join-Path $repo 'changed.txt')
    Package
    $asset = Join-Path $downloads 'NTPods-Native-AAC.zip'
    $archive = [IO.Compression.ZipFile]::OpenRead($asset)
    try {
        foreach ($name in @($files) + @('preflight.ps1', 'verify-bundle.ps1', 'tools/devcon.exe', 'README.md', 'TESTING.md', 'LICENSE', 'licenses/notice.txt', 'SHA256SUMS.txt')) {
            if (-not $archive.GetEntry($name)) { throw "Missing bundle file: $name" }
        }
        if ($archive.Entries.FullName -match 'fdk|eld-encoder|\.pfx$|\.key$') { throw 'Unexpected private or AAC-ELD payload.' }
    } finally { $archive.Dispose() }
    $line = Get-Content -LiteralPath (Join-Path $downloads 'SHA256SUMS.txt')
    if ($line -ne "$((Get-FileHash -LiteralPath $asset).Hash.ToLowerInvariant())  NTPods-Native-AAC.zip") { throw 'Wrong release asset checksum.' }
    $unpacked = Join-Path $fixture 'unpacked'
    Expand-Archive -LiteralPath $asset -DestinationPath $unpacked
    $verify = Join-Path $unpacked 'verify-bundle.ps1'
    & $verify -Bundle $unpacked
    & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File $verify -Bundle $unpacked
    if ($LASTEXITCODE) { throw 'Windows PowerShell bundle verification failed.' }
    $sums = Join-Path $unpacked 'SHA256SUMS.txt'
    $original = Get-Content -LiteralPath $sums
    $extra = Join-Path $unpacked 'runtime/unexpected.dll'
    Set-Content -LiteralPath $extra -Value 'unlisted'
    Reject { & $verify -Bundle $unpacked } '*Unlisted bundle file*'
    Remove-Item -LiteralPath $extra
    Set-Content -LiteralPath $sums -Value $original[1..($original.Count - 1)]
    Reject { & $verify -Bundle $unpacked } '*Unlisted bundle file*'
    Set-Content -LiteralPath $sums -Value $original
    Add-Content -LiteralPath $sums -Value $original[0]
    Reject { & $verify -Bundle $unpacked } '*duplicate bundle path*'
    Set-Content -LiteralPath $sums -Value "$hash  ../escape.exe"
    Reject { & $verify -Bundle $unpacked } '*Invalid or duplicate*'
    Set-Content -LiteralPath $sums -Value $original
    Add-Content -LiteralPath (Join-Path $unpacked 'runtime/ntpodsd.exe') -Value 'tampered'
    Reject { & $verify -Bundle $unpacked } '*changed or is missing*'
    Write-Host 'Playback package: stale builds, AAC-ELD payloads and tampering rejected; ZIP contents and checksums passed.'
} finally {
    $resolved = [IO.Path]::GetFullPath($fixture)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path -Leaf $resolved) -notlike 'ntpods-playback-package-*') { throw 'Unsafe test cleanup path.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
