# Product

<!-- impeccable:product-schema 1 -->

## Platform

Native Windows desktop (WinUI 3).

## Users and purpose

NTPods provides AirPods controls, battery levels, AAC/AAC-ELD playback and a high-quality microphone for Windows 11 22H2 or newer, x64. Playback stays in stereo while the microphone is active. Supported features vary by model; see [model support](docs/WINDOWS-MODEL-SUPPORT.md).

## Capabilities and constraints

Keep the Rust daemon, driver interfaces and WinUI behavior. Track confirmed settings, pending changes, unsupported features and errors separately. Use control availability and accessible help for routine status. Keep active microphone use, recovery and actionable errors visible.

NTPods replaces the supported AirPods playback driver and offers AAC and AAC-ELD through the same output device. It does not require Alternative A2DP. Installation backs up the previous driver for restoration on uninstall. Visual changes must not alter Bluetooth connections, audio streaming, startup registration or device settings.

## Appearance

Use LibrePods' Android rewrite with Apple appearance as the reference: one centered settings page, existing device artwork and grouped controls. Keep the NTPods name and native Windows keyboard, focus and accessibility behavior.

## Operating context

Users open the app to check their earbuds or change a setting, then return to listening. Support light, dark and system appearance. Put less-used settings in expandable groups.

## Reference

The LibrePods rewrite reference is commit `34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c`. Device state and commands use the shared Rust/C# IPC contract. Visual tests must not send settings changes to the earbuds.
