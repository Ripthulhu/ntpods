#requires -Version 7.0
[CmdletBinding()]
param([switch]$IncludeEld, [string]$Out)

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$sourceCommit = & git -C $repo rev-parse HEAD
if ($LASTEXITCODE) { throw 'Cannot identify the source commit.' }
$sourceDirty = [bool](& git -C $repo status --porcelain)
if ($LASTEXITCODE) { throw 'Cannot inspect the source checkout.' }
if (-not $Out) {
    $folder = if ($IncludeEld) { 'experimental-playback' } else { 'native-playback' }
    $Out = Join-Path $PSScriptRoot "..\build-output\$folder"
}
$Out = [IO.Path]::GetFullPath($Out)
if (-not $IncludeEld) {
    foreach ($file in 'fdk-aac.dll', 'ntpods-eld-encoder.exe', 'NOTICE-FDK.txt') {
        if (Test-Path -LiteralPath (Join-Path $Out "runtime\$file")) {
            throw 'The output contains optional AAC-ELD files. Choose a different output directory.'
        }
    }
}
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs = & $vswhere -latest -products '*' -version '[17.0,18.0)' -property installationPath
$msbuild = & $vswhere -latest -products '*' -version '[17.0,18.0)' -requires Microsoft.Component.MSBuild -find 'MSBuild\Current\Bin\amd64\MSBuild.exe' | Select-Object -First 1
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$bash = Join-Path $env:ProgramFiles 'Git\bin\bash.exe'
if ($IncludeEld) { $python = (Get-Command python.exe -ErrorAction Stop).Source }
if (-not $vs -or -not $msbuild) { throw 'Install Visual Studio 2022 C++ Build Tools and the Windows Driver Kit.' }
foreach ($tool in $cargo, $bash) {
    if (-not (Test-Path -LiteralPath $tool)) { throw "Missing build tool: $tool" }
}

function Invoke-Checked([string]$Tool, [string[]]$Arguments) {
    & $Tool @Arguments
    if ($LASTEXITCODE) { throw "$Tool failed ($LASTEXITCODE)." }
}

function Build-Driver([string]$Project, [string]$Name, [string[]]$Extra = @()) {
    $directory = Join-Path $Out "obj\$Name"
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    Invoke-Checked $msbuild (@($Project, '/t:Rebuild', '/p:Configuration=Release',
        '/p:Platform=x64', '/p:WindowsTargetPlatformVersion=10.0.26100.0',
        '/p:NTPodsExperimentalPlayback=true', '/p:EnableInf2cat=false', '/p:SignMode=Off',
        "/p:OutDir=$directory\", "/p:IntDir=$directory\", '/m', '/nr:false', '/v:minimal') + $Extra)
}

$oldPath = $env:PATH
$oldRustFlags = $env:RUSTFLAGS
$oldEncoderTestPath = $env:NTPODS_ELD_ENCODER_TEST_PATH
New-Item -ItemType Directory -Path $Out -Force | Out-Null
Push-Location $repo
try {
    Import-Module (Join-Path $vs 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
    Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64'
    $tests = Join-Path $Out 'tests'
    New-Item -ItemType Directory -Path $tests -Force | Out-Null
    foreach ($test in @(
        @{ Name = 'aap-playback'; Source = 'windows/drivers/aap/tests/playback_transport_test.cpp' },
        @{ Name = 'aap-concurrency'; Source = 'windows/drivers/aap/tests/receive_concurrency_test.cpp' },
        @{ Name = 'render-bridge'; Source = 'windows/drivers/mic/tests/render_bridge_test.cpp' },
        @{ Name = 'mic-bridge'; Source = 'windows/drivers/mic/tests/mic_bridge_test.cpp' }
    )) {
        $binary = Join-Path $tests ($test.Name + '.exe')
        $object = Join-Path $tests ($test.Name + '.obj')
        Invoke-Checked 'cl.exe' @('/nologo', '/W4', '/WX', '/EHsc', '/std:c++17',
            "/Fe:$binary", "/Fo:$object", $test.Source)
        Invoke-Checked $binary @()
    }

    Build-Driver 'windows/drivers/aap/NTPodsAAP.vcxproj' 'NTPodsAAP'
    Build-Driver 'windows/drivers/mic/Common/SamplesCommon.vcxproj' 'SamplesCommon'
    $commonLibrary = Join-Path $Out 'obj\SamplesCommon\SamplesCommon.lib'
    Build-Driver 'windows/drivers/mic/AudioCodec/Driver/AudioCodec.vcxproj' 'AudioCodec' @(
        "/p:NTPodsSamplesCommonLibrary=$commonLibrary")

    if ($IncludeEld) {
        & (Join-Path $PSScriptRoot 'encoder\build.ps1')
        Invoke-Checked $python @((Join-Path $PSScriptRoot 'encoder\test.py'))
    }
    Invoke-Checked $bash @('windows/daemon/fetch-ffmpeg.sh')
    $ffmpeg = Join-Path $repo 'windows\daemon\vendor\ffmpeg\bin'
    $env:PATH = "$ffmpeg;$env:PATH"
    $aacTest = Join-Path $tests 'aac-shim.exe'
    Invoke-Checked 'cl.exe' @('/nologo', '/W4', '/WX', '/std:c11', '/MD', '/O2',
        '/external:W0', '/external:Iwindows/daemon/vendor/ffmpeg/include',
        "/Fe:$aacTest", "/Fo:$(Join-Path $tests 'aac-shim.obj')",
        'windows/daemon/tests/aac_shim_test.c', '/link',
        '/LIBPATH:windows/daemon/vendor/ffmpeg/lib', 'avcodec.lib', 'avutil.lib')
    Invoke-Checked $aacTest @()
    $target = Join-Path $Out 'cargo'
    $env:RUSTFLAGS = '-C target-feature=+crt-static'
    $cargoArguments = @('--locked', '--manifest-path', 'windows/daemon/Cargo.toml',
        '--features', 'experimental-playback', '--target-dir', $target)
    Invoke-Checked $cargo (@('test') + $cargoArguments)
    if ($IncludeEld) {
        $env:NTPODS_ELD_ENCODER_TEST_PATH = Join-Path $PSScriptRoot 'encoder\build\Release\ntpods-eld-encoder.exe'
        Invoke-Checked $cargo (@('test') + $cargoArguments + @(
            'playback::encoder::tests::streams_real_worker_and_drops_it', '--', '--ignored', '--exact'))
    }
    Invoke-Checked $cargo (@('build', '--release') + $cargoArguments)

    $runtime = Join-Path $Out 'runtime'
    New-Item -ItemType Directory -Path $runtime -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $target 'release\ntpodsd.exe') -Destination $runtime -Force
    Get-ChildItem -LiteralPath $ffmpeg -Filter '*.dll' | Copy-Item -Destination $runtime -Force
    Copy-Item -LiteralPath (Join-Path $ffmpeg '..\LICENSE-ffmpeg.txt') -Destination $runtime -Force
    Copy-Item -LiteralPath (Join-Path $repo 'THIRD_PARTY.md') -Destination $runtime -Force
    if ($IncludeEld) {
        foreach ($file in 'ntpods-eld-encoder.exe', 'fdk-aac.dll') {
            Copy-Item -LiteralPath (Join-Path $PSScriptRoot "encoder\build\Release\$file") -Destination $runtime -Force
        }
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'encoder\NOTICE-FDK.txt') -Destination $runtime -Force
    }
    foreach ($driver in @(
        @{ Name = 'NTPodsAAP'; Destination = 'driver' },
        @{ Name = 'AudioCodec'; Destination = 'driver-mic-acx' }
    )) {
        $destination = Join-Path $Out $driver.Destination
        New-Item -ItemType Directory -Path $destination -Force | Out-Null
        foreach ($extension in 'sys', 'inf') {
            $source = Join-Path $Out ("obj\{0}\{0}.{1}" -f $driver.Name, $extension)
            Copy-Item -LiteralPath $source -Destination $destination -Force
        }
        $inf2cat = "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.0.26100.0\x86\Inf2Cat.exe"
        Invoke-Checked $inf2cat @("/driver:$destination", '/os:10_NI_X64,10_GE_X64', '/uselocaltime')
    }
    $currentCommit = & git -C $repo rev-parse HEAD
    if ($LASTEXITCODE -or $currentCommit -ne $sourceCommit) { throw 'Source commit changed during the build.' }
    $dirtyAfterBuild = [bool](& git -C $repo status --porcelain)
    if ($LASTEXITCODE) { throw 'Cannot inspect the source checkout.' }
    $manifest = [ordered]@{
        commit = $sourceCommit
        dirty = $sourceDirty -or $dirtyAfterBuild
        codecs = @(if ($IncludeEld) { 'aac'; 'eld' } else { 'aac' })
        signing = 'unsigned-development-package'
        files = @(foreach ($folder in 'runtime', 'driver', 'driver-mic-acx') {
            Get-ChildItem -LiteralPath (Join-Path $Out $folder) -Recurse -File | ForEach-Object {
                @{ path = [IO.Path]::GetRelativePath($Out, $_.FullName).Replace('\', '/'); sha256 = (Get-FileHash -LiteralPath $_.FullName).Hash }
            }
        })
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $Out 'build-manifest.json') -Encoding UTF8
    Write-Host "Local experimental build: $Out"
    Write-Host 'The drivers are unsigned and have not been installed. This folder is not a release package.'
} finally {
    $env:PATH = $oldPath
    $env:RUSTFLAGS = $oldRustFlags
    $env:NTPODS_ELD_ENCODER_TEST_PATH = $oldEncoderTestPath
    Pop-Location
}
