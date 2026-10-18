---
version: 1
slug: "windows-winui-ntpods-winui-pages-devicepage-xaml"
primary_target: "windows/winui/NTPods.WinUI/Pages/DevicePage.xaml"
related_targets: ["windows/winui/NTPods.WinUI/MainWindow.xaml","windows/winui/NTPods.WinUI/Pages/SettingsPage.xaml"]
---

# Device settings

Mode: Operate. Native WinUI main window, device page and app settings.

## Layout

Use one centered column: 640 DIP of controls plus 24 DIP padding on each side.
Place the device name and connection status above the artwork and battery rings.
Follow with listening modes and grouped settings. Put less-used controls in
native expanders and app settings in the device header's three-dot menu.
The scroll area spans the window, including the side margins. Keep the scrollbar
at the window edge. Symmetric header action columns and reserved name/status
heights keep the device centered when Connect or Repair appears.

Use LibrePods' Apple appearance: a black or light gray page, white device artwork,
rounded charcoal or white groups, inset dividers, green switches and battery rings.
Keep native Windows text, keyboard controls and accessibility.
Expander headings use the same normal-weight body style as setting labels.
The name and connection status use single-line ellipsis; settings labels wrap.

Supported unreported preferences allow an explicit choice. Unknown booleans use
an unselected native On/Off chooser in the same 144 DIP slot as the reported
switch. Keep sent choices separate from device reports in tooltips and accessible
help. Require both hold actions before Apply. Saved choices stay with their pair
and never send automatically when loaded.

Require explicit Apply for audiogram changes; debounce tuning gestures and
preserve untouched settings from the current pair's confirmed profile.

## Listening modes

Modes share one draggable glass capsule. Adaptive expands within the rail to
expose its continuous 0–100 strength range. Show Off according to the model and
Allow Off setting; hide it while Adaptive is expanded. Keep requested and
confirmed values separate, coalesce changes during dragging and send the final
value on release. Preserve the geometry and motion documented in DESIGN.md.

## State changes

Use the existing section and artwork transition helpers. Animate actual device
and visibility changes without restarting motion on every snapshot. Keep the
decoded image visible while replacement artwork loads, and keep cached readings
tied to the correct pair. Dim unavailable artwork independently of stale battery
values. Earbuds use three readouts; headphones use one centered readout.

Pair AirPods is available in the header menu and empty state without the daemon.
Automatic pairing uses the bottom connection card with the advertised model or discovered name,
shared fallback artwork, Connect and close. It works from the tray and never
activates automatically. The card stays white with dark text and a blue Connect
button in light and dark appearance; high contrast uses Windows system colors.
The card uses the island's staged, interruptible motion
and reduced-motion preference. A lid-open report can show the card before the
pairing gesture. Connect shows the gesture while discovery finds a single AirPods
endpoint to pair automatically. Keep device lists out of this card. If several
AirPods are available, wait and ask the user to close the other cases. A Classic
invitation already identifies its displayed endpoint and can pair it directly.
Allow 60 seconds for discovery and pairing. Transient pairing failures keep the gesture visible.
Closing the case cancels and dismisses its lid invitation; reopening can show it again, even during dismissal. Keep unrelated Classic and reconnect cards independent.
Manual discovery remains available for renamed devices. Suppress duplicate lid
invitations and repeated Classic invitations. Pause expiry while interacting or
pairing. Pause background Classic scans while a card or dialog is open, but allow
its requested scan. Inhibit discovery during microphone capture and shutdown.

## Checks

Hardware checks use a collapsed expander and manual Refresh. Show saved per-ear
results with neutral text and separate device timestamps from the time read.
Cached flags do not establish a current fault.

Keep backend behavior, localization, themes and error handling intact. Visual
tests must use an isolated renderer and must not send commands to AirPods.
Compare light, dark, narrow, disconnected and unsupported-feature states. Include
switching pairs, long translated labels, stale readings and interrupted motion.
See docs/WINDOWS-VALIDATION.md for test results.

Reference: LibrePods rewrite commit `34076c85`, `android/imgs/settings-1.png`,
`settings-2.png` and `presentation/components/apple/NoiseControlSettings.kt`.
