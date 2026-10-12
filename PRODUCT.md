# Product

<!-- impeccable:product-schema 1 -->

## Platform

Native Windows desktop (WinUI 3).

## Users and purpose

NTPods gives Windows users access to AirPods controls, battery information and a virtual microphone while retaining stereo playback. Initial hardware validation uses Windows 11, an Intel AX200 and AirPods Pro 3.

## Capabilities and constraints

Keep the existing Rust daemon, driver interfaces and WinUI behavior. Device-reported settings, pending writes, missing capabilities and errors remain distinguishable through state, control availability and accessible help. Keep routine status copy out of the main layout. Active microphone use, recovery and actionable errors remain visible. Hardware support and production signing are separate from interface quality.

The existing Alternative A2DP playback driver must remain intact. This redesign does not alter Bluetooth, microphone streaming, startup registration or device settings.

## Brand commitments

The user requested LibrePods' Android rewrite with Apple appearance, adapted to NTPods. They chose one focused settings page and direct implementation from that reference. Keep the NTPods name and existing device artwork. Use native Windows keyboard, focus and accessibility behavior.

## Operating context

Users open the app to check their earbuds or change a setting, then return to listening. Support light, dark and system appearance. Retain app settings and all existing feature controls; use progressive disclosure for less frequent adjustments.

## Evidence on hand

The LibrePods rewrite reference is commit `34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c`. Existing device state and controls are available through the shared Rust/C# IPC contract. Visual QA must not send settings changes to the earbuds.
