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
$pcmHeader = Join-Path $source 'libSYS/include/machine_type.h'
$pcmOriginal = "typedef SHORT INT_PCM;`n#define MAXVAL_PCM MAXVAL_SGL`n#define MINVAL_PCM MINVAL_SGL`n#define WAV_BITS 16`n#define SAMPLE_BITS 16"
$pcmWide = "typedef LONG INT_PCM;`n#define MAXVAL_PCM MAXVAL_DBL`n#define MINVAL_PCM MINVAL_DBL`n#define WAV_BITS 32`n#define SAMPLE_BITS 32"
$header = [IO.File]::ReadAllText($pcmHeader).Replace("`r`n", "`n")
if ($header.Split(@($pcmOriginal), [StringSplitOptions]::None).Count -ne 2) {
    throw 'The pinned FDK PCM definition has changed.'
}
$notice = 'Third-Party Modified Version of the Fraunhofer FDK AAC Codec Library for Android. NTPods, 2026-10-20: enabled existing 32-bit PCM paths and added a PCM-width ABI check.'
[IO.File]::WriteAllText($pcmHeader, "/* $notice */`n" + $header.Replace($pcmOriginal, $pcmWide), [Text.UTF8Encoding]::new($false))
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'pcm-abi.c') -Destination (Join-Path $source 'ntpods-pcm-abi.c') -Force
Add-Content -LiteralPath (Join-Path $source 'CMakeLists.txt') -Value "`ntarget_sources(fdk-aac PRIVATE ntpods-pcm-abi.c)`nset_target_properties(fdk-aac PROPERTIES OUTPUT_NAME ntpods-fdk-q31)" -Encoding UTF8
[IO.File]::WriteAllText((Join-Path $source 'NTPods-MODIFICATIONS.txt'), $notice + "`n", [Text.UTF8Encoding]::new($false))
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$cmake = & $vswhere -latest -products '*' -version '[17.0,18.0)' -find 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe' | Select-Object -First 1
if (-not $cmake) { throw 'Visual Studio 2022 CMake tools are required.' }
$build = Join-Path $PSScriptRoot 'build'
& $cmake -S $PSScriptRoot -B $build -G 'Visual Studio 17 2022' -A x64
if ($LASTEXITCODE) { throw "CMake configuration failed ($LASTEXITCODE)." }
& $cmake --build $build --config Release --parallel
if ($LASTEXITCODE) { throw "Encoder build failed ($LASTEXITCODE)." }
# Preserve the verified archive's other entries and include the exact modified source.
$sourceBundle = Join-Path $build 'FDK-AAC-source.zip'
Copy-Item -LiteralPath $archive -Destination $sourceBundle -Force
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::Open($sourceBundle, [IO.Compression.ZipArchiveMode]::Update)
try {
    foreach ($relative in 'libSYS/include/machine_type.h', 'CMakeLists.txt', 'ntpods-pcm-abi.c', 'NTPods-MODIFICATIONS.txt') {
        $entryName = "fdk-aac-$revision/$relative"
        $entry = $zip.GetEntry($entryName)
        if ($entry) { $entry.Delete() }
        [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, (Join-Path $source $relative), $entryName) | Out-Null
    }
} finally { $zip.Dispose() }
Set-Content -LiteralPath (Join-Path $build 'FDK-AAC-source.sha256') -Value (Get-FileHash -LiteralPath $sourceBundle -Algorithm SHA256).Hash -Encoding ASCII
Write-Host "AAC-ELD encoder: $build\Release\ntpods-eld-encoder.exe"
