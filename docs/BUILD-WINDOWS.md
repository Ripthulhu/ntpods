# Experimental Windows build

> [!WARNING]
> Running these drivers requires Windows **Test Mode (test signing)**, which
> weakens normal driver-signature enforcement. They are not Microsoft
> production-signed; the installer signs them locally. Enabling Test Mode on an
> everyday or work PC is a bad idea. Use a dedicated test PC. Save your BitLocker
> recovery key before changing Test Mode or Secure Boot.

Baseline: `188e45df20259b616ce155be812086259d1a2488`, Windows 11 x64 / ACX only.
The current source version is `0.1.26`. The installed version is tracked in
`WINDOWS-VALIDATION.md`.
Builds do not install or load drivers. Hardware acceptance and Microsoft signing
remain separate gates. Read `THIRD_PARTY.md` before distributing anything.

## Toolchain

- Visual Studio 2022 Build Tools 17.14.41, C++ workload, x64 Spectre libraries,
  and **Component.Microsoft.Windows.DriverKit.BuildTools**. For full VS2022 the
  integration component is **Microsoft.Windows.DriverKit**.
- Windows SDK 26100.4188 and WDK 26100.6584, both exposing build directory `10.0.26100.0`.
- Visual Studio 2026 Build Tools 18.10.2, WinUI build workload
  **Microsoft.VisualStudio.Workload.UniversalBuildTools**. .NET 10 requires
  MSBuild 18; VS2022 is retained for the 26100 driver toolset.
- .NET SDK 10.0.401 (`global.json`), Rust 1.95.0 (`rust-toolchain.toml`), Git Bash.
- WiX 5.0.2 and `WixToolset.UI.wixext/5.0.2`, `WixToolset.Util.wixext/5.0.2`.

Install development tools through their official installers. No boot setting,
test certificate, or NTPods driver is needed to compile.

```powershell
dotnet tool install --global wix --version 5.0.2
wix extension add -g WixToolset.UI.wixext/5.0.2 WixToolset.Util.wixext/5.0.2
.\windows\build.ps1
```

The script verifies FFmpeg, uses locked Rust/NuGet dependencies, runs unit tests,
builds both drivers from source, generates catalogs, builds WinUI, and assembles
`windows/build-output/NTPods-experimental.msi`. It never takes driver binaries
from committed `prebuilt` folders. The manifest records artifact hashes and
whether local source modifications were included. Build inputs are pinned;
generated timestamps can still change output hashes.

Startup checks use a private temporary registry key and a restricted MSI session
that cannot install files or run setup scripts. New MSI installs leave startup
off unless selected. Upgrades preserve the current app setting, including off.

For a major app-only upgrade with unchanged, already installed drivers, pass
`APPONLYUPGRADE=1` to MSI and keep `Drivers,MicAcx` in `ADDLOCAL`. This skips driver
replacement and its reboot request while preserving driver removal on uninstall.
The flag has no effect on fresh installs or repairs. Verify driver package hashes
and the installed drivers before choosing this path.

## Publishing experimental builds

Binary uploads are off by default. The repository variable
`NTPODS_PUBLISH_BUILDS` must equal `true` before the Windows workflow can upload
binary artifacts or publish a release. Keep it off while the licensing issue in
`THIRD_PARTY.md` is unresolved. Tests and compilation still run with uploads off.

When publishing is enabled, the workflow packages the MSI, a ZIP for manual
installation, checksums and build metadata. Every release starts with
[`WINDOWS-RELEASE-NOTES.md`](WINDOWS-RELEASE-NOTES.md), which is also included in
the ZIP. Keep its Test Mode warning visible when sharing download links.

Nightlies reserve versions `0.2.<github.run_number>` so each new run can upgrade
the previous build. A rerun retains its original run number. Versioned release
tags use `windows-vX.Y.Z`; start at `windows-v0.3.0` or higher so the MSI version
sorts after every `0.2.x` nightly. These are experimental builds regardless of
whether they came from a branch or a version tag.

## Development installation

Use a test setup already configured for test-signed drivers. This installer does
not change Secure Boot, Memory Integrity, or Test Mode. Both MSI and ZIP install
executables and elevated helpers under `%ProgramFiles%\NTPods`. The installing
account's SID is bound to both driver control devices; SYSTEM is also allowed.
This build accepts ordinary local/domain account SIDs, not Entra-only SIDs.

Test Mode must remain enabled while using the NTPods drivers. Secure Boot can
prevent enabling it, and changing boot settings can prompt for a BitLocker
recovery key. See [Microsoft's test-signing documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/the-testsigning-boot-configuration-option).
After uninstalling the drivers, turn Test Mode off and restore any Secure Boot
setting you changed. Uninstalling NTPods does not reset boot settings.

The ACX hardware ID is `ROOT\NTPodsMicACX` (service `NTPodsMicACX`), avoiding the
generic Microsoft sample device identity. PortCls is not shipped by this build.
Helper tasks can restart only the unique NTPods AAP service device, never the
Bluetooth radio, and rename only this virtual input without resetting Windows
audio services.

The AAP INF binds only Apple's dedicated service UUID. Installation and removal
select only NTPods driver packages; the Alternative A2DP/Luculent audio-sink
driver and its codec configuration are outside every setup action.

Remove MSI installations through Windows Settings > Apps. ZIP installations
include an elevated `uninstall.ps1`, which removes programs, tasks, driver-store
packages, SID policy and development certificates. Saved settings/history remain
in the user's `%LOCALAPPDATA%\NTPods` and can be deleted from that user's normal
unelevated session. Development toolchains remain installed for future rebuilds.
Boot settings are never automatically changed.

Before replacing drivers, setup exports the installed NTPods packages and saves
their access policy under `%ProgramData%\NTPods\driver-rollback`. Both new packages
must stage successfully before setup unbinds the old drivers. An installation
failure restores those packages, microphone devices and access policy. MSI keeps
the backup until the transaction commits, including upgrades and repairs. A failed
restoration leaves the backup in place and reports its location. Recovery that
requires a Windows restart cannot finish until that restart occurs.

Normal deployment must use Microsoft-signed driver packages verified on a clean
Secure Boot/Memory Integrity system. The current installer is deliberately a
test-signing installer and must not be described as a production release.
