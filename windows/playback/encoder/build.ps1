[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$revision = 'cac04476081a23c870fed05c36228cd02c5e7c3d'
$expectedHash = '1B572FCB5E2A81AA0447A92CA95BFE4979F3EDFC09918E0627B726BCD0165FF1'
$vendor = Join-Path $PSScriptRoot 'vendor'
$archive = Join-Path $vendor "fdk-aac-$revision.zip"
New-Item -ItemType Directory -Path $vendor -Force | Out-Null
if (-not (Test-Path -LiteralPath $archive)) {
    Invoke-WebRequest "https://codeload.github.com/mstorsjo/fdk-aac/zip/$revision" -OutFile "$archive.part"
    Move-Item -LiteralPath "$archive.part" -Destination $archive
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $expectedHash) {
    throw 'FDK source archive hash does not match.'
}
# Re-extract the verified source so edits in the vendor cache cannot enter a build.
$vendorRoot = [IO.Path]::GetFullPath($vendor).TrimEnd('\')
$source = [IO.Path]::GetFullPath((Join-Path $vendorRoot "fdk-aac-$revision"))
if (-not $source.StartsWith($vendorRoot + '\', [StringComparison]::OrdinalIgnoreCase) -or
    (Split-Path -Leaf $source) -ne "fdk-aac-$revision" -or
    ((Get-Item -LiteralPath $vendorRoot -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw 'Unsafe encoder source path.'
}
if (Test-Path -LiteralPath $source) {
    $entries = @((Get-Item -LiteralPath $source -Force)) + @(Get-ChildItem -LiteralPath $source -Recurse -Force)
    if ($entries | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) { throw 'Encoder source contains a reparse point.' }
    Remove-Item -LiteralPath $source -Recurse -Force
}
Expand-Archive -LiteralPath $archive -DestinationPath $vendor -Force
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$cmake = & $vswhere -latest -products '*' -version '[17.0,18.0)' -find 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe' | Select-Object -First 1
if (-not $cmake) { throw 'Visual Studio 2022 CMake tools are required.' }
$build = Join-Path $PSScriptRoot 'build'
& $cmake -S $PSScriptRoot -B $build -G 'Visual Studio 17 2022' -A x64
if ($LASTEXITCODE) { throw "CMake configuration failed ($LASTEXITCODE)." }
& $cmake --build $build --config Release --parallel
if ($LASTEXITCODE) { throw "Encoder build failed ($LASTEXITCODE)." }
Write-Host "AAC-ELD encoder: $build\Release\ntpods-eld-encoder.exe"
