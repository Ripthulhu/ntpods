$ErrorActionPreference = 'Stop'
$build = Join-Path $PSScriptRoot '..\build.ps1'
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($build, [ref]$null, [ref]$parseErrors)
if ($parseErrors) { throw $parseErrors[0] }
# Exercise parameter binding without starting compilers or changing build outputs.
$bind = [scriptblock]::Create($ast.ParamBlock.Extent.Text + "`nreturn `$Version")
if (& $bind -Out $PSScriptRoot) { throw 'The local build must keep its existing version when no override is supplied.' }
foreach ($version in '0.1.26', '0.2.1', '0.2.65535', '255.255.65535', '0.0.0') {
    if ((& $bind -Out $PSScriptRoot -Version $version) -cne $version) { throw "Valid version rejected: $version" }
}
foreach ($version in '', '0.2', '0.2.1.0', '0.2.-1', '0.2.1-preview', '0.2.1;bad',
    ' 0.2.1', "0.2.1`n", '256.0.0', '0.256.0', '0.2.65536', '999999999999.2.1', 'a.b.c') {
    $rejected = $false
    try { $null = & $bind -Out $PSScriptRoot -Version $version } catch [Management.Automation.ParameterBindingException] { $rejected = $true }
    if (-not $rejected) { throw "Invalid version accepted: $version" }
}
Write-Host 'Build version validation passed (19 cases).'
