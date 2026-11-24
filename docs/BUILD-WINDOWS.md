# Build the Windows dongle preview

Install PowerShell 7, Python 3, Rust via rustup, the .NET SDK pinned in
`global.json` and Visual Studio 2026 Build Tools with the WinUI workload.
Rust uses the version pinned in `rust-toolchain.toml`. A Visual C++ toolchain
and the Windows SDK are required for the bundled SQLite build. Install x64
LLVM with libclang for the spatial renderer. Set `LIBCLANG_PATH` to its
`bin` directory if bindgen cannot find it.

From the repository root:

```powershell
./windows/build.ps1
```

The script checks the firmware protocol lock, localization, daemon and
WinUI settings, and the Rust/C# IPC contract. It builds a portable x64 ZIP
under `windows/build-output`. Extract the ZIP before launching
`ntpods-winui.exe`.

The preview uses a separate application instance and named pipe namespace
so it can connect to its own daemon when a direct Bluetooth build is present.
The branch uses the dongle's USB audio and control services.

The build compiles and tests the Windows spatial renderer from `windows/spatial`
using its Cargo lockfile. On first build, AudioNimbus downloads the matching Steam
Audio SDK. The ZIP includes the renderer, its runtime and both verified SADIE II
profiles so comparison works without a separate download. [Spatial audio setup](PICO-DONGLE.md#spatial-audio)

The 3D listening room loads its bundled HTML, JavaScript and shaders through
WebView2. Run `node --test windows/tests/SpatialRoom/*.test.cjs` for the
coordinate, picking and group-drag checks used in CI. For a
release screen check, open Spatial audio in the unpackaged preview, orbit
the room, move one speaker, and confirm its native azimuth and elevation
follow. **Reset view** should return the camera and keep that placement.
