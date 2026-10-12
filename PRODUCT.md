# Product

<!-- impeccable:product-schema 1 -->

## Platform

Native Windows desktop (WinUI 3).

## Users and purpose

NTPods provides AirPods controls, battery levels and a microphone for Windows while keeping playback in stereo. Hardware testing uses Windows 11, an Intel AX200 and AirPods Pro 3; first-generation AirPods Pro have also been tested for core controls.

## Capabilities and constraints

Keep the Rust daemon, driver interfaces and WinUI behavior. Track confirmed settings, pending changes, unsupported features and errors separately. Use control availability and accessible help for routine status. Keep active microphone use, recovery and actionable errors visible.

Preserve existing A2DP playback drivers and codec settings, including Alternative A2DP. Visual changes must not alter Bluetooth connections, microphone streaming, startup registration or device settings.

## Appearance

Use LibrePods' Android rewrite with Apple appearance as the reference: one centered settings page, existing device artwork and grouped controls. Keep the NTPods name and native Windows keyboard, focus and accessibility behavior.

## Operating context

Users open the app to check their earbuds or change a setting, then return to listening. Support light, dark and system appearance. Put less-used settings in expandable groups.

## Reference

The LibrePods rewrite reference is commit `34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c`. Device state and commands use the shared Rust/C# IPC contract. Visual tests must not send settings changes to the earbuds.
