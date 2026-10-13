# Build only: this never installs drivers, trusts certificates, or changes boot options.
[CmdletBinding()]
param(
    [string]$Out = (Join-Path $PSScriptRoot 'build-output'),
    [ValidateScript({
        if ($_ -notmatch '\A[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,5}\z') {
            throw 'Version must contain three numeric parts: major.minor.build.'
        }
        $value = [version]$_
        if ($value.Major -gt 255 -or $value.Minor -gt 255 -or $value.Build -gt 65535) {
            throw 'Version exceeds the Windows Installer limits: 255.255.65535.'
        }
        $true
    })]
    [string]$Version
)
$ErrorActionPreference = 'Stop'
& python.exe (Join-Path $PSScriptRoot 'tests/test-localization.py')
if ($LASTEXITCODE) { throw 'Localization checks failed.' }
& (Join-Path $PSScriptRoot 'tests\test-build-version.ps1')
$repo = Split-Path -Parent $PSScriptRoot
$Out = [IO.Path]::GetFullPath($Out)
$uiVersionArguments = @()
$msiVersionArguments = @{}
if ($Version) {
    # AssemblyVersion reserves 65535; keep the full MSI build number in FileVersion.
    $assemblyVersion = ([version]$Version).ToString(2) + '.0.0'
    $uiVersionArguments = @("-p:Version=$Version", "-p:FileVersion=$Version.0", "-p:AssemblyVersion=$assemblyVersion")
    $msiVersionArguments.Version = $Version
}
function Run([string]$tool, [string[]]$arguments) {
    & $tool @arguments
    if ($LASTEXITCODE) { throw "$tool failed ($LASTEXITCODE)." }
}
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$driverBuild = & $vswhere -latest -products '*' -version '[17.0,18.0)' -requires Microsoft.Component.MSBuild -find 'MSBuild\Current\Bin\amd64\MSBuild.exe' | Select-Object -First 1
$uiBuild = & $vswhere -latest -products '*' -version '[18.0,19.0)' -requires Microsoft.Component.MSBuild -find 'MSBuild\Current\Bin\MSBuild.exe' | Select-Object -First 1
if (-not $driverBuild -or -not $uiBuild) { throw 'Install VS2022 C++/WDK Build Tools and VS2026 WinUI Build Tools; see docs/BUILD-WINDOWS.md.' }
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$bash = Join-Path $env:ProgramFiles 'Git\bin\bash.exe'
$inf2cat = "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.0.26100.0\x86\Inf2Cat.exe"
foreach ($tool in $cargo, $bash, $inf2cat) { if (-not (Test-Path -LiteralPath $tool)) { throw "Missing build tool: $tool" } }
New-Item -ItemType Directory -Path $Out -Force | Out-Null
$oldPath = $env:PATH
Push-Location $repo
try {
    $driverVs = & $vswhere -latest -products '*' -version '[17.0,18.0)' -property installationPath
    Import-Module (Join-Path $driverVs 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
    Enter-VsDevShell -VsInstallPath $driverVs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64'
    $tests = Join-Path $Out 'tests'
    New-Item -ItemType Directory -Path $tests -Force | Out-Null
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/EHsc', '/std:c++17', '/MT', "/Fe:$tests\audio-binding.exe", "/Fo:$tests\audio-binding.obj", 'windows/installer/audio-binding.cpp', '/link', 'setupapi.lib', 'newdev.lib')
    Run (Join-Path $tests 'audio-binding.exe') @('--self-test')
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/EHsc', '/std:c++17', "/Fe:$tests\audio-inf-target.exe", "/Fo:$tests\audio-inf-target.obj", 'windows/drivers/audio/tests/inf_target_test.cpp', '/link', 'setupapi.lib')
    Run (Join-Path $tests 'audio-inf-target.exe') @('windows/drivers/audio/NTPodsAudio.inf')
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/TC', "/Fe:$tests\aap-policy.exe", "/Fo:$tests\aap-policy.obj", 'windows/drivers/aap/tests/policy_test.c')
    Run (Join-Path $tests 'aap-policy.exe') @()
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/EHsc', '/std:c++17', "/Fe:$tests\aap-concurrency.exe", "/Fo:$tests\aap-concurrency.obj", 'windows/drivers/aap/tests/receive_concurrency_test.cpp')
    Run (Join-Path $tests 'aap-concurrency.exe') @()
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/TC', "/Fe:$tests\aap-inf-target.exe", "/Fo:$tests\aap-inf-target.obj", 'windows/drivers/aap/tests/inf_target_test.c', '/link', 'setupapi.lib')
    Run (Join-Path $tests 'aap-inf-target.exe') @('windows/drivers/aap/NTPodsAAP.inf')
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/EHsc', '/std:c++17', "/Fe:$tests\mic-bridge.exe", "/Fo:$tests\mic-bridge.obj", 'windows/drivers/mic/tests/mic_bridge_test.cpp')
    Run (Join-Path $tests 'mic-bridge.exe') @()
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/EHsc', '/std:c++17', "/Fe:$tests\render-bridge.exe", "/Fo:$tests\render-bridge.obj", 'windows/drivers/mic/tests/render_bridge_test.cpp')
    Run (Join-Path $tests 'render-bridge.exe') @()
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/EHsc', '/std:c++17', "/Fe:$tests\packet-constraints.exe", "/Fo:$tests\packet-constraints.obj", 'windows/drivers/mic/tests/packet_constraints_test.cpp', '/link', 'setupapi.lib')
    Run (Join-Path $tests 'packet-constraints.exe') @('windows/drivers/audio/NTPodsAudio.inf', 'windows/drivers/mic/AudioCodec/Driver/AudioCodec.inf')
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/EHsc', '/std:c++17', "/Fe:$tests\aap-playback.exe", "/Fo:$tests\aap-playback.obj", 'windows/drivers/aap/tests/playback_transport_test.cpp')
    Run (Join-Path $tests 'aap-playback.exe') @()
    Run $bash @('windows/daemon/fetch-ffmpeg.sh')
    $env:PATH = "$(Join-Path $repo 'windows\daemon\vendor\ffmpeg\bin');$env:PATH"
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/TC', '/external:Iwindows/daemon/vendor/ffmpeg/include', '/external:W0', "/Fe:$tests\eld-shim.exe", "/Fo:$tests\eld-shim.obj", 'windows/daemon/tests/eld_shim_test.c', '/link', '/LIBPATH:windows/daemon/vendor/ffmpeg/lib', 'avcodec.lib', 'avutil.lib', 'swresample.lib')
    Run (Join-Path $tests 'eld-shim.exe') @()
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/std:c11', '/MD', '/O2', '/external:W0', '/external:Iwindows/daemon/vendor/ffmpeg/include', "/Fe:$tests\aac-shim.exe", "/Fo:$tests\aac-shim.obj", 'windows/daemon/tests/aac_shim_test.c', '/link', '/LIBPATH:windows/daemon/vendor/ffmpeg/lib', 'avcodec.lib', 'avutil.lib', 'Avrt.lib')
    Run (Join-Path $tests 'aac-shim.exe') @()
    $aacWorkerDir = Join-Path $repo 'windows\daemon\target\release'
    New-Item -ItemType Directory -Path $aacWorkerDir -Force | Out-Null
    Run 'cl.exe' @('/nologo', '/W4', '/WX', '/std:c11', '/MT', '/O2', '/external:W0', '/external:Iwindows/daemon/vendor/ffmpeg/include', "/Fe:$aacWorkerDir\ntpods-aac-encoder.exe", "/Fo:$tests\aac-worker.obj", 'windows/daemon/src/playback/aac_worker.c', '/link', '/LIBPATH:windows/daemon/vendor/ffmpeg/lib', 'avcodec.lib', 'avutil.lib', 'Avrt.lib')
    & windows/playback/encoder/build.ps1
    Run 'python.exe' @('windows/playback/encoder/test.py')
    Run $cargo @('test', '--locked', '--manifest-path', 'windows/ipc/Cargo.toml')
    & windows/tests/test-ipc.ps1
    Run 'dotnet' @('run', '--project', 'windows/tests/Startup/Startup.csproj', '--configuration', 'Release')
    Run 'dotnet' @('run', '--project', 'windows/tests/ListeningMode/ListeningMode.csproj', '--configuration', 'Release')
    Run 'dotnet' @('run', '--project', 'windows/tests/DeviceSettings/DeviceSettings.csproj', '--configuration', 'Release')
    Run 'dotnet' @('run', '--project', 'windows/tests/SensorUi/SensorUi.csproj', '--configuration', 'Release')
    Run 'dotnet' @('run', '--project', 'windows/tests/ClientSession/ClientSession.csproj', '--configuration', 'Release')
    Run 'dotnet' @('run', '--project', 'windows/tests/IslandMotion/IslandMotion.csproj', '--configuration', 'Release')
    Run 'dotnet' @('run', '--project', 'windows/tests/Hotkey/Hotkey.csproj', '--configuration', 'Release')
    Run $cargo @('test', '--locked', '--manifest-path', 'windows/daemon/Cargo.toml', '--features', 'experimental-playback')
    $oldEncoderTestPath = $env:NTPODS_ELD_ENCODER_TEST_PATH
    $oldAacEncoderTestPath = $env:NTPODS_AAC_ENCODER_TEST_PATH
    try {
        $env:NTPODS_ELD_ENCODER_TEST_PATH = Join-Path $repo 'windows/playback/encoder/build/Release/ntpods-eld-encoder.exe'
        $env:NTPODS_AAC_ENCODER_TEST_PATH = Join-Path $aacWorkerDir 'ntpods-aac-encoder.exe'
        Run $cargo @('test', '--locked', '--manifest-path', 'windows/daemon/Cargo.toml', '--features', 'experimental-playback', 'playback::encoder::tests::streams_real_worker_and_drops_it', '--', '--ignored', '--exact')
    } finally {
        $env:NTPODS_ELD_ENCODER_TEST_PATH = $oldEncoderTestPath
        $env:NTPODS_AAC_ENCODER_TEST_PATH = $oldAacEncoderTestPath
    }
    $oldRustFlags = $env:RUSTFLAGS
    try {
        $env:RUSTFLAGS = '-C target-feature=+crt-static'
        Run $cargo @('build', '--release', '--locked', '--manifest-path', 'windows/daemon/Cargo.toml', '--features', 'experimental-playback')
    } finally { $env:RUSTFLAGS = $oldRustFlags }
    foreach ($project in 'windows/drivers/aap/NTPodsAAP.vcxproj', 'windows/drivers/mic/AudioCodec/Driver/AudioCodec.sln', 'windows/drivers/audio/NTPodsAudio.vcxproj') {
        Run $driverBuild @($project, '/p:Configuration=Release', '/p:Platform=x64', '/p:WindowsTargetPlatformVersion=10.0.26100.0', '/p:NTPodsExperimentalPlayback=true', '/p:EnableInf2cat=false', '/p:SignMode=Off', '/m', '/nr:false', '/v:minimal')
    }
    $packages = Join-Path $Out 'driver-packages'
    New-Item -ItemType Directory -Path (Join-Path $packages 'tools') -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $tests 'audio-binding.exe') -Destination (Join-Path $packages 'tools\audio-binding.exe') -Force
    foreach ($driver in @(
        @{ Directory = 'driver'; Root = 'windows/drivers/aap'; Name = 'NTPodsAAP' },
        @{ Directory = 'driver-mic-acx'; Root = 'windows/drivers/mic/AudioCodec/Driver'; Name = 'AudioCodec' },
        @{ Directory = 'driver-audio'; Root = 'windows/drivers/audio'; Name = 'NTPodsAudio' }
    )) {
        $sys = Get-ChildItem -LiteralPath $driver.Root -Recurse -Filter "$($driver.Name).sys" | Where-Object { $_.FullName -match '\\x64\\Release\\' -and $_.FullName -notmatch '\\prebuilt\\' } | Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if (-not $sys) { throw "Missing freshly built $($driver.Name).sys" }
        $inf = Join-Path $sys.Directory.FullName "$($driver.Name).inf"
        if (-not (Test-Path -LiteralPath $inf)) { $inf = Join-Path $driver.Root "$($driver.Name).inf" }
        $package = Join-Path $packages $driver.Directory
        New-Item -ItemType Directory -Path $package -Force | Out-Null
        Copy-Item -LiteralPath $sys.FullName, $inf -Destination $package -Force
        # StampInf uses local dates; matching its clock avoids false postdating near midnight.
        Run $inf2cat @("/driver:$package", '/os:10_X64', '/uselocaltime')
        if ($driver.Name -eq 'NTPodsAAP') {
            Run (Join-Path $tests 'aap-inf-target.exe') @((Join-Path $package 'NTPodsAAP.inf'))
        }
        if ($driver.Name -eq 'NTPodsAudio') {
            Run (Join-Path $tests 'audio-inf-target.exe') @((Join-Path $package 'NTPodsAudio.inf'))
        }
    }
    Run $uiBuild (@('windows/winui/NTPods.WinUI/NTPods.WinUI.csproj', '-restore', '-t:Build', '-p:Configuration=Release', '-p:Platform=x64', '-p:VisualStudioVersion=18.0', '-p:RestorePackagesWithLockFile=true', '-p:RestoreLockedMode=true', '-nr:false', '-v:minimal') + $uiVersionArguments)
    $ui = Get-ChildItem 'windows/winui/NTPods.WinUI/bin/x64/Release' -Recurse -Filter ntpods-winui.exe | Where-Object { $_.FullName -notmatch '\\publish\\' } | Select-Object -First 1
    if (-not $ui) { throw 'Missing built WinUI executable.' }
    & windows/installer/test-setup.ps1
    Run "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'windows/installer/test-setup.ps1')
    & windows/installer/test-driver-rollback.ps1
    Run "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'windows/installer/test-driver-rollback.ps1')
    & windows/installer/test-audio-driver.ps1
    Run "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'windows/installer/test-audio-driver.ps1')
    & windows/installer/test-fix-driver.ps1
    Run "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'windows/installer/test-fix-driver.ps1')
    & windows/installer/make-dist.ps1 -Out (Join-Path $Out 'dist') -DaemonExe windows/daemon/target/release/ntpodsd.exe -WinUIDir $ui.Directory.FullName -DriverPackages $packages
    $env:PATH = "$env:USERPROFILE\.dotnet\tools;$env:PATH"
    & windows/installer/msi/build-msi.ps1 -Dist (Join-Path $Out 'dist') -Out (Join-Path $Out 'NTPods-experimental.msi') @msiVersionArguments
    & windows/installer/msi/test-startup.ps1 -Msi (Join-Path $Out 'NTPods-experimental.msi')
    $manifest = [ordered]@{
        baseline = '188e45df20259b616ce155be812086259d1a2488'
        commit = (& git rev-parse HEAD)
        dirty = [bool](& git status --porcelain)
        rust = (& $cargo --version)
        dotnet = (& dotnet --version)
        sdkWdk = '10.0.26100.0'
        signing = 'unsigned-development-package'
        msi = @{ path = 'NTPods-experimental.msi'; sha256 = (Get-FileHash -LiteralPath (Join-Path $Out 'NTPods-experimental.msi') -Algorithm SHA256).Hash }
        sources = @(& git -c core.quotePath=false ls-files --cached --others --exclude-standard | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | ForEach-Object { @{ path = $_; sha256 = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash } })
        files = @(Get-ChildItem (Join-Path $Out 'dist') -Recurse -File | ForEach-Object { @{ path = $_.FullName.Substring($Out.Length + 1); sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash } })
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $Out 'build-manifest.json') -Encoding UTF8
} finally { $env:PATH = $oldPath; Pop-Location }
