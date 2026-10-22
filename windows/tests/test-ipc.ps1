$ErrorActionPreference = 'Stop'
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
$cargo = if ($cargoCommand) { $cargoCommand.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
if (-not (Test-Path -LiteralPath $cargo)) { throw 'Install Rust with Cargo before running IPC checks.' }
$dotnet = (Get-Command dotnet -ErrorAction Stop).Source
$manifest = Join-Path $PSScriptRoot '..\ipc\Cargo.toml'
$project = Join-Path $PSScriptRoot 'IpcContract\IpcContract.csproj'
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\', '/')
$runDirectory = Join-Path $tempRoot ('ntpods-ipc-tests-' + [Guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($runDirectory)
try {
    $rustEvents = Join-Path $runDirectory 'rust-events.ndjson'
    $csharpEvents = Join-Path $runDirectory 'csharp-events.ndjson'
    & $cargo run --quiet --locked --manifest-path $manifest --example mic_wire_fixtures -- emit $rustEvents
    if ($LASTEXITCODE -ne 0) { throw 'Rust fixture generation failed.' }
    & $dotnet run --project $project --configuration Release -- $rustEvents $csharpEvents
    if ($LASTEXITCODE -ne 0) { throw 'C# IPC contract checks failed.' }
    & $cargo run --quiet --locked --manifest-path $manifest --example mic_wire_fixtures -- check $csharpEvents
    if ($LASTEXITCODE -ne 0) { throw 'Rust/C# IPC roundtrip checks failed.' }
    & $cargo run --quiet --locked --manifest-path $manifest --example companion_preferences_fixtures -- emit $rustEvents
    if ($LASTEXITCODE -ne 0) { throw 'Rust preference fixture generation failed.' }
    & $dotnet run --project $project --configuration Release -- --preferences $rustEvents $csharpEvents
    if ($LASTEXITCODE -ne 0) { throw 'C# preference contract checks failed.' }
    & $cargo run --quiet --locked --manifest-path $manifest --example companion_preferences_fixtures -- check $csharpEvents
    if ($LASTEXITCODE -ne 0) { throw 'Rust/C# preference roundtrip checks failed.' }
    & $cargo run --quiet --locked --manifest-path $manifest --example dongle_wire_fixtures -- emit $rustEvents
    if ($LASTEXITCODE -ne 0) { throw 'Rust dongle fixture generation failed.' }
    & $dotnet run --project $project --configuration Release -- --dongle $rustEvents $csharpEvents
    if ($LASTEXITCODE -ne 0) { throw 'C# dongle contract checks failed.' }
    & $cargo run --quiet --locked --manifest-path $manifest --example dongle_wire_fixtures -- check $csharpEvents
    if ($LASTEXITCODE -ne 0) { throw 'Rust/C# dongle roundtrip checks failed.' }
} finally {
    $resolved = [IO.Path]::GetFullPath($runDirectory)
    $prefix = $tempRoot + [IO.Path]::DirectorySeparatorChar + 'ntpods-ipc-tests-'
    if (-not $resolved.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to remove a fixture directory outside the expected temporary path.'
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
