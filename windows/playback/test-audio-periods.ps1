# Queries periods; -Initialize never starts audio, -HoldSeconds runs only a bounded silent stream.
[CmdletBinding(DefaultParameterSetName = 'Query')]
param(
    [Parameter(Mandatory, ParameterSetName = 'Query')]
    [Parameter(Mandatory, ParameterSetName = 'Hold')] [string]$EndpointId,
    [Parameter(ParameterSetName = 'Query')] [switch]$Initialize,
    [Parameter(Mandatory, ParameterSetName = 'Hold')] [ValidateRange(1,120)] [int]$HoldSeconds,
    [Parameter(Mandatory, ParameterSetName = 'SelfTest')] [switch]$SelfTest,
    [string]$Out = (Join-Path $env:TEMP 'NTPods-audio-periods')
)
$ErrorActionPreference = 'Stop'
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$installation = & $vswhere -latest -products '*' -version '[17.0,18.0)' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $installation) { throw 'VS2022 C++ tools are required.' }
Import-Module (Join-Path $installation 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $installation -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null
$null = New-Item -ItemType Directory -Force -Path $Out
$folder = (Resolve-Path -LiteralPath $Out).Path
$exe = Join-Path $folder 'audio-periods.exe'
$arguments = @('/nologo', '/W4', '/WX', '/EHsc', '/std:c++17', '/MT', '/O2', "/Fe:$exe", "/Fo:$folder\audio-periods.obj", (Join-Path $PSScriptRoot 'audio-periods.cpp'), '/link', 'ole32.lib', 'uuid.lib', 'cfgmgr32.lib')
& cl.exe @arguments | Out-Host
if ($LASTEXITCODE) { throw "Compilation failed ($LASTEXITCODE)." }
& $exe --self-test | Out-Host
if ($LASTEXITCODE) { throw "Self-test failed ($LASTEXITCODE)." }
if (-not $SelfTest) {
    $probeArguments = @($EndpointId)
    if ($Initialize) { $probeArguments += '--initialize' }
    if ($HoldSeconds) { $probeArguments += @('--hold-seconds', $HoldSeconds) }
    & $exe @probeArguments
    if ($LASTEXITCODE) { throw "Audio period diagnostic failed ($LASTEXITCODE)." }
}
