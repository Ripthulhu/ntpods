#requires -Version 5.1
[CmdletBinding()]
param([string]$Bundle = $PSScriptRoot)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $Bundle).Path.TrimEnd('\') + '\'
$seen = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
foreach ($line in Get-Content -LiteralPath (Join-Path $root 'SHA256SUMS.txt')) {
    if ($line -notmatch '^([0-9a-fA-F]{64})  (.+)$') { throw 'Malformed bundle checksum.' }
    $hash, $relative = $Matches[1], $Matches[2]
    $path = [IO.Path]::GetFullPath((Join-Path $root $relative))
    if (-not $path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) -or -not $seen.Add($path)) {
        throw "Invalid or duplicate bundle path: $relative"
    }
    if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or (Get-FileHash -LiteralPath $path).Hash -ne $hash) {
        throw "Bundle file changed or is missing: $relative"
    }
}
if (-not $seen.Count) { throw 'The bundle checksum list is empty.' }
foreach ($file in Get-ChildItem -LiteralPath $root -Recurse -File -Force) {
    if ($file.FullName -ieq (Join-Path $root 'SHA256SUMS.txt')) { continue }
    if (-not $seen.Contains($file.FullName)) { throw "Unlisted bundle file: $($file.Name)" }
}
Write-Host "Verified $($seen.Count) bundle files."
