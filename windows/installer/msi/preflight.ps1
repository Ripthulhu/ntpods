# Embedded in the MSI custom action; never loaded from the installation directory.
$ErrorActionPreference = 'Stop'
$root = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'NTPods'
if (-not (Test-Path -LiteralPath $root)) { exit 0 }
$pending = New-Object 'System.Collections.Generic.Stack[string]'
$pending.Push($root)
$trusted = @('S-1-5-18', 'S-1-5-32-544', 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
$unsafeRights = [Security.AccessControl.FileSystemRights]'Write,Delete,DeleteSubdirectoriesAndFiles,ChangePermissions,TakeOwnership'
while ($pending.Count) {
    $path = $pending.Pop()
    $entry = Get-Item -LiteralPath $path -Force
    if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'NTPods installation contains a reparse point. Remove it before running setup.' }
    $acl = Get-Acl -LiteralPath $path
    if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin $trusted) {
        throw 'The existing NTPods installation is not administrator-owned. Setup will not execute its elevated helpers.'
    }
    foreach ($rule in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -eq 'Allow' -and $rule.IdentityReference.Value -notin $trusted -and
            -not ($rule.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -and
            ($rule.FileSystemRights -band $unsafeRights)) { throw 'The existing NTPods installation is user-writable. Setup will not execute its elevated helpers.' }
    }
    if ($entry.PSIsContainer) {
        foreach ($child in Get-ChildItem -LiteralPath $path -Force) { $pending.Push($child.FullName) }
    }
}
Stop-NTPods
