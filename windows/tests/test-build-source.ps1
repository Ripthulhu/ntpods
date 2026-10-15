$ErrorActionPreference = 'Stop'
$build = Join-Path $PSScriptRoot '..\build.ps1'
$parseErrors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($build, [ref]$null, [ref]$parseErrors)
if ($parseErrors) { throw $parseErrors[0] }
# Exercise the production checks without running compilers or touching the working checkout.
foreach ($name in 'Get-BuildSourceIdentity', 'Assert-BuildSourceUnchanged') {
    $function = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
    }, $true)
    if (-not $function) { throw "Missing build check: $name" }
    . ([scriptblock]::Create($function.Extent.Text))
}
$fixture = [IO.Path]::GetFullPath((Join-Path ([IO.Path]::GetTempPath()) ('ntpods-build-source-' + [guid]::NewGuid().ToString('N'))))
function Invoke-FixtureGit([string[]]$Arguments) {
    & git -C $fixture @Arguments
    if ($LASTEXITCODE) { throw "Fixture Git command failed: $Arguments" }
}
function Commit-Fixture {
    Invoke-FixtureGit @('-c', 'user.name=NTPods-test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '--quiet', '-m', 'fixture')
}
function Reject-SourceChange($Identity) {
    try { Assert-BuildSourceUnchanged $fixture $Identity }
    catch { if ($_.Exception.Message -like 'Source changed during the build.*') { return }; throw }
    throw 'A source change was accepted.'
}
New-Item -ItemType Directory -Path (Join-Path $fixture 'build-output') -Force | Out-Null
try {
    Invoke-FixtureGit @('init', '--quiet')
    Invoke-FixtureGit @('config', 'core.autocrlf', 'false')
    $source = Join-Path $fixture 'source with spaces.txt'
    $trackedOutput = Join-Path $fixture 'build-output/tracked.txt'
    [IO.File]::WriteAllText((Join-Path $fixture '.gitignore'), "build-output/`n")
    [IO.File]::WriteAllText($source, 'original')
    [IO.File]::WriteAllText($trackedOutput, 'tracked output')
    Invoke-FixtureGit @('add', '.')
    Invoke-FixtureGit @('add', '-f', 'build-output/tracked.txt')
    Commit-Fixture
    $clean = Get-BuildSourceIdentity $fixture
    if ($clean.dirty -or $clean.sources.Count -ne 3) { throw 'Clean source identity is wrong.' }
    $originalHash = (Get-FileHash -LiteralPath $source).Hash
    if (($clean.sources | Where-Object path -eq 'source with spaces.txt').sha256 -ne $originalHash) { throw 'Source content was not hashed.' }
    Assert-BuildSourceUnchanged $fixture $clean

    [IO.File]::WriteAllText((Join-Path $fixture 'build-output/ignored.bin'), 'generated')
    Assert-BuildSourceUnchanged $fixture $clean
    [IO.File]::WriteAllText($trackedOutput, 'changed generated file')
    Reject-SourceChange $clean
    Invoke-FixtureGit @('restore', 'build-output/tracked.txt')

    $untracked = Join-Path $fixture 'new source.txt'
    [IO.File]::WriteAllText($untracked, 'new')
    Reject-SourceChange $clean
    Remove-Item -LiteralPath $untracked
    Remove-Item -LiteralPath $source
    Reject-SourceChange $clean
    Invoke-FixtureGit @('restore', 'source with spaces.txt')

    [IO.File]::WriteAllText($source, 'first edit')
    Reject-SourceChange $clean
    $dirty = Get-BuildSourceIdentity $fixture
    if (-not $dirty.dirty) { throw 'Local edits were not recorded.' }
    Assert-BuildSourceUnchanged $fixture $dirty
    [IO.File]::WriteAllText($source, 'second edit')
    Reject-SourceChange $dirty
    [IO.File]::WriteAllText($source, 'first edit')
    Invoke-FixtureGit @('add', '.')
    Reject-SourceChange $dirty
    Commit-Fixture
    Reject-SourceChange $clean
    Reject-SourceChange $dirty
    if (($clean.sources | Where-Object path -eq 'source with spaces.txt').sha256 -ne $originalHash) { throw 'The starting source snapshot was changed.' }
    Assert-BuildSourceUnchanged $fixture (Get-BuildSourceIdentity $fixture)
    Write-Host 'Build source identity: ignored output accepted; edits, additions, deletion, staging and commits during a build rejected.'
} finally {
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $fixture.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path -Leaf $fixture) -notlike 'ntpods-build-source-*') { throw 'Unsafe test cleanup path.' }
    Remove-Item -LiteralPath $fixture -Recurse -Force
}
