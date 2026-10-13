# Third-party provenance and redistribution notes

This experimental Windows build starts from NTPods commit
`188e45df20259b616ce155be812086259d1a2488`. NTPods and its LibrePods-derived
application/protocol code carry the repository's GPL-3.0 license. Keep `LICENSE`
with binaries and provide the corresponding source and build instructions.

The native three-band EQ and additional AirPods control mappings use protocol
research from LibrePods' Android rewrite at
[`34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c`](https://github.com/librepods-org/librepods/tree/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c),
licensed GPL-3.0-or-later. The Windows implementation retains NTPods' existing
transport. See `docs/WINDOWS-FEATURES.md` for packet sources and limits.

The WinUI settings redesign follows that rewrite's Apple appearance and the
gesture geometry in `presentation/components/apple/NoiseControlSettings.kt`.
The three `ic_adaptive`, `ic_transparency`, and `ic_noise_cancellation` PNGs in
`windows/winui/NTPods.WinUI/Assets` come from its `android/app/src/main/res/drawable`
directory at the same pinned commit. They are rendered as monochrome icons by
WinUI; source pixels are preserved. The island's `island_airpods_pro_buds.png`
is an unchanged copy of `res-apple/drawable/img_airpods_pro_2_buds_cropped.png`
from that commit. Its layout follows `presentation/overlays/IslandContent.kt`.
`airpods_max.png` is an unchanged copy of that revision's
`res-apple/drawable/img_airpods_max.png`.
Other AirPods product artwork remains from the original NTPods asset set.

The shared `GroupExpander` template adapts the WinUI `Expander` template in
`Microsoft.WinUI/Themes/generic.xaml` from the pinned
[Microsoft.WindowsAppSDK 1.7.260224002 package](https://www.nuget.org/packages/Microsoft.WindowsAppSDK/1.7.260224002).
Its native header and clipping behavior follow the WinUI 1.7
[template](https://github.com/microsoft/microsoft-ui-xaml/blob/a37e4e2c44582980aad0f9d91b11230a119cffac/src/controls/dev/Expander/Expander.xaml)
and [control source](https://github.com/microsoft/microsoft-ui-xaml/blob/a37e4e2c44582980aad0f9d91b11230a119cffac/src/controls/dev/Expander/Expander.cpp).
Copyright (c) Microsoft Corporation. All rights reserved. The MIT notice from
that source revision is retained in `licenses/Microsoft-UI-Xaml-MIT.txt`.

The listening selector's rounded-rectangle refraction, depth, seven spectral
samples and directional rim follow AndroidLiquidGlass (Backdrop) 2.0.1,
[`65ab177e90e5c1d8c62e70cf7755841982da65f6`](https://github.com/Kyant0/AndroidLiquidGlass/tree/65ab177e90e5c1d8c62e70cf7755841982da65f6).
`ListeningModeLens.cs` adapts `backdrop/src/commonMain/kotlin/com/kyant/backdrop/internal/Shaders.kt`;
`ListeningModeGlass.cs` implements the material with Win2D displacement maps.
Copyright 2025 Kyant. The original code is licensed under Apache-2.0; its license
is retained in `licenses/AndroidLiquidGlass-Apache-2.0.txt`. The port precomputes
the three resting widths and interpolates their fields during width transitions.
It preserves the original negative 36-DIP refraction, 12-DIP edge depth and
spectral weights. It does not ship the Android library.

Microsoft.Graphics.Win2D 1.3.2 supplies the GPU drawing layer and is pinned in
the WinUI project and lock file. This version supports the existing Windows
App SDK 1.7; Win2D 1.4.0 requires the newer App SDK 1.8 WinUI package. Microsoft's
Win2D source license is retained in `licenses/Win2D-MIT.txt`. The NuGet package's
legacy license URL currently redirects to an unrelated Microsoft Learn page;
confirm the binary package's distribution terms before a public release.

Paired-device BLE battery parsing follows `linux/battery.hpp`,
`linux/ble/bleutils.cpp`, and `linux/airpods_packets.h` at that same LibrePods
revision. The Windows implementation uses Windows CNG for AES and current-user
DPAPI for key storage. Its address-resolution test uses a public input/output
fixture from CAPod's `RPACheckerTest`; no CAPod implementation is included.
Island motion follows the staged content and non-bouncing expansion in
`presentation/overlays/IslandContent.kt` and `IslandWindow.kt`.

Live head tracking follows the same rewrite's `AppleDevice.kt` service selection,
`AACPManager.kt` RTBuddy motion decoding, and Apple `HeadTrackingScreen.kt` graph.
The Windows view plots the same two raw movement channels; it does not interpret
them as calibrated angles or implement spatial audio.

## Microsoft ACX sample

`windows/drivers/mic` and the shared code used by `windows/drivers/audio` contain derivatives of Microsoft's ACX AudioCodec sample,
with Microsoft copyright notices in the source. The upstream
[Windows-driver-samples license](https://github.com/microsoft/Windows-driver-samples/blob/d5569c08aa2818c6240744bb47a00f67f20fdb54/LICENSE)
is **Microsoft Public License (MS-PL), not MIT**. Its full text is retained in
`licenses/Microsoft-Public-License.txt`. This revision pins the license text;
NTPods' original Microsoft sample revision is unknown.

NTPods imported the driver in
[`48ce5c4f092c7d0138a9d6e810305060cae51218`](https://github.com/arctumn/ntpods/commit/48ce5c4f092c7d0138a9d6e810305060cae51218),
including `Common/MicPipe.{h,cpp}` and modifications to the capture circuit and
stream engine. Its driver README called the Microsoft sample MIT-licensed,
while the root README and license identify NTPods as GPL-3.0. The Git history
through `188e45d` contains no driver-specific exception or dual-license grant
that reconciles those statements with Microsoft's MS-PL terms.

The combined driver's licensing remains unresolved. Resolution needs the relevant
authors' permission or replacement of the affected implementation. No such
permission has been obtained. The Microsoft notice and publication of this fork's
source and experimental binaries do not resolve the conflict or establish
permission to relicense or redistribute the combined driver.

## FFmpeg

The AAC-ELD decoder and AAC-LC playback encoder dynamically link
FFmpeg 7.1 LGPL-3.0-or-later shared libraries, pinned to
the BtbN monthly build `autobuild-2026-07-31-14-10`, source revision
`n7.1.5-12-g1fdbca85aa`, and archive SHA-256
`0f376f96fb38554ccefb1b2ae9c7c6a7b351f0e60a372b38262c320e8392c5d0`.
`windows/daemon/fetch-ffmpeg.sh` verifies the archive and cached extracted files.
The distribution retains FFmpeg's LGPL text and this notice.
Keep the DLLs replaceable and supply the exact corresponding FFmpeg
source and build scripts when distributing binaries; source-retrieval pointers
alone are not a claim of complete license compliance.
Experimental downloads include the exact FFmpeg source and matching BtbN build
recipes, with pinned hashes. Dependency source archives are not bundled; see
[source archive notes](docs/FFMPEG-SOURCES.md) for the contents and limitations.

## AAC-ELD playback encoder

AAC-ELD playback uses a separate worker linked to FDK-AAC 2.0.3. The worker's
original code is MIT-licensed; FDK retains its own license. Packages include
`licenses/LICENSE-encoder.txt`, `licenses/NOTICE-FDK.txt` and the complete pinned
source archive, `licenses/FDK-AAC-source.zip`. The GPL backend exchanges audio
with the worker through pipes and does not link FDK. This separation does not
establish definitive GPL compatibility, and FDK grants no patent license.
See [the encoder README](windows/playback/encoder/README.md) for the revision,
hash and licensing references.

AAC-ELD packet framing follows
[USBPods' macOS capture research](https://github.com/wasdwasd0105/USBPods-Pico2W/blob/6286ab1618ec1143d828b1fc377d8e22c449e567/aac-eld-apple.md).
The implementation uses that protocol description; it does not include
Alternative A2DP code or binaries.

## Other dependencies and assets

Cargo.lock and the WinUI project pin application dependencies. Preserve their
license notices when creating a public release. `devcon.exe` is a Microsoft tool;
its bundled binary's redistribution terms must also be confirmed before public
distribution. Apple product names and artwork remain their owners' property.
This build is not affiliated with Apple, Microsoft, or LibrePods.
