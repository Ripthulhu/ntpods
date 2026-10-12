---
version: 1
slug: "windows-winui-ntpods-winui-popup-islandview-xaml"
primary_target: "windows/winui/NTPods.WinUI/Popup/IslandView.xaml"
related_targets: ["windows/winui/NTPods.WinUI/Popup/IslandWindow.cs", "windows/winui/NTPods.WinUI/Popup/IslandMotion.cs", "windows/winui/NTPods.WinUI/Popup/TrayMenuView.xaml"]
---

# Battery island

Mode: Operate. Native Windows desktop, WinUI 3. Refine the user's chosen
LibrePods Apple appearance and preserve primary-monitor placement.

The capsule expands gently, reveals the device and event first, then the three
battery readings. Left, Right, and Case remain visible during listening-mode
messages. A charging bolt appears only with a live reported value. Missing
readings use an em dash; cached readings are dimmed and dated.

Use the existing artwork, native text and theme resources. The island remains
non-activating and frameless. Respect high contrast and Windows reduced motion.
Animate ring changes from the displayed position. Battery reports update the
open island without restarting dismissal or producing repeated notifications.
The tray offers Show batteries for an explicit glance at the latest readings.
Animation updates follow WinUI rendering frames while moving. Use a one-shot
timer only for dismissal while idle, and update the native window region only
when its rounded pixel bounds change.

Reference: LibrePods rewrite 34076c85, IslandContent.kt and IslandWindow.kt.
The test-only visual harness runs outside the production project and does not
connect to the daemon or send device commands. Hardware freshness and sleep
behavior require separate live validation.

## Verification

Native captures checked connected, listening-mode, nearby-charging, and cached
states in dark and light appearances. Labels load correctly, zero and unknown
values remain distinct, and long names truncate without covering readings.
The user-reported charging alignment was corrected with centered vector paths
in the island and main battery display. Motion and presence tests pass. Live
radio behavior is a separate acceptance check; no sample readings ship in the
production app.
