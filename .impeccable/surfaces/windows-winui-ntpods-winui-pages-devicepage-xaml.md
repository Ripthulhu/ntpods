---
version: 1
slug: "windows-winui-ntpods-winui-pages-devicepage-xaml"
primary_target: "windows/winui/NTPods.WinUI/Pages/DevicePage.xaml"
related_targets: ["windows/winui/NTPods.WinUI/MainWindow.xaml","windows/winui/NTPods.WinUI/Pages/SettingsPage.xaml"]
---

# Device settings

Mode: Operate. Native WinUI main window, device page and app settings.

## Layout

Use one centered settings column, up to 640 DIP wide, with 24 DIP side padding.
Place the device name and connection status above the artwork and battery rings.
Follow with listening modes and grouped settings. Put less-used controls in
native expanders and app settings in the device header's three-dot menu.

Use LibrePods' Apple appearance: a black or light gray page, white device artwork,
rounded charcoal or white groups, inset dividers, green switches and battery rings.
Keep native Windows text, keyboard controls and accessibility.

## Listening modes

Modes share one draggable glass capsule. Adaptive expands within the rail to
expose its continuous 0–100 strength range. Show Off according to the model and
Allow Off setting; hide it while Adaptive is expanded. Keep requested and
confirmed values separate, coalesce changes during dragging and send the final
value on release. Preserve the geometry and motion documented in DESIGN.md.

## Checks

Keep backend behavior, localization, themes and error handling intact. Visual
tests must use an isolated renderer and must not send commands to AirPods.
Compare light, dark, narrow, disconnected and unsupported-feature states.
See docs/WINDOWS-VALIDATION.md for test results.

Reference: LibrePods rewrite commit `34076c85`, `android/imgs/settings-1.png`,
`settings-2.png` and `presentation/components/apple/NoiseControlSettings.kt`.
