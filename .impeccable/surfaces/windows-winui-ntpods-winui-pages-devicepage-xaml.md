---
version: 1
slug: "windows-winui-ntpods-winui-pages-devicepage-xaml"
primary_target: "windows/winui/NTPods.WinUI/Pages/DevicePage.xaml"
related_targets: ["windows/winui/NTPods.WinUI/MainWindow.xaml","windows/winui/NTPods.WinUI/Pages/SettingsPage.xaml"]
---

# Device settings redesign

Mode: Operate. Scope: the native Windows main window, device page and application settings. The user chose a single focused settings page built directly from LibrePods' Apple appearance.

## Direction contract

THESIS: Give AirPods a familiar settings home. Replace the two-column dashboard with one centered reading column and inset groups.

OWN-WORLD: Black or cool light gray canvas, white device artwork, rounded charcoal or white groups, fine inset dividers, green switches and battery rings. Segoe UI keeps Windows text and accessibility behavior intact.

STORY: Recognize the connected AirPods, read each battery, change listening mode, then open less frequent sound and device adjustments. Unconfirmed and unavailable states remain explicit.

FIRST VIEWPORT: Quiet top navigation; centered device name and connection status; three prominent device parts with compact battery rings; neutral segmented listening modes and grouped listening switches. Content caps at 640 DIP, with 24 DIP side padding and a viewport that stays centered at narrower widths.

FORM: The user-pinned LibrePods Apple design supersedes seed bb142380's assigned direction and challengers. Progressive disclosure uses native expanders and green toggles. Listening modes share a draggable selected capsule. Off appears only when the device reports it enabled. Adaptive expands within the same pill to reveal its strength range; changes apply on release and remain pending until confirmed. Preserve native keyboard and accessibility controls; no glass renderer.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Constraints

Keep the existing backend, native accessible controls, live localization, theme choice, safety gates and error states. No test data on the live device page and no hardware-setting changes during visual review. Reference images: LibrePods rewrite android/imgs/settings-1.png and settings-2.png at commit 34076c85.

The user's subsequent screenshot of the LibrePods device list is the authority for listening-mode interaction: expanded Adaptive, movable thumb and hidden unavailable Off. Its source is `presentation/components/apple/NoiseControlSettings.kt`, lines 218–753.

## Finish result

The fresh native visual review returned `ship` on 2026-10-11 after inspecting both user screenshots and eight final captures. The centered viewport, switch alignment, enabled colors, and embedded Adaptive control passed. DESIGN.md and the schema-2 design sidecar record the built system. Release build and selector geometry/safety tests passed. Live setting requests timed out; hardware control changes remain unverified. This is a running build preview, not an installed upgrade.
