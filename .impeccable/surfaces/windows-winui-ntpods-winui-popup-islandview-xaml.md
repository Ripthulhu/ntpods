---
version: 1
slug: "windows-winui-ntpods-winui-popup-islandview-xaml"
primary_target: "windows/winui/NTPods.WinUI/Popup/IslandView.xaml"
related_targets: ["windows/winui/NTPods.WinUI/Popup/IslandWindow.cs", "windows/winui/NTPods.WinUI/Popup/IslandMotion.cs", "windows/winui/NTPods.WinUI/Popup/TrayMenuView.xaml"]
---

# Battery island

Mode: Operate. Native WinUI popup with LibrePods' Apple appearance,
shown on the primary monitor.

The capsule expands gently, reveals the device and event first, then the three
battery readings. Left, Right, and Case remain visible during listening-mode
messages. A charging bolt appears only with a live reported value. Missing
readings use an em dash; cached readings are dimmed and dated.

Use the existing artwork, native text and theme resources. The island remains
non-activating and frameless. Respect high contrast and Windows reduced motion.
Animate ring changes from the displayed position. Battery reports update the
open island without restarting dismissal or producing repeated notifications.
The tray's Show batteries action opens the latest readings.
Animation updates follow WinUI rendering frames while moving. Use a one-shot
timer only for dismissal while idle, and update the native window region only
when its rounded pixel bounds change.

Reference: LibrePods rewrite 34076c85, IslandContent.kt and IslandWindow.kt.
Run visual tests separately from the app, without connecting to the daemon.

## Checks

Compare connected, listening-mode, nearby-charging and cached states in light
and dark appearances. Check localized labels, zero and missing values, and long
names. Center the charging bolt in each ring. Run the motion and presence tests;
check radio freshness and sleep behavior with hardware.
