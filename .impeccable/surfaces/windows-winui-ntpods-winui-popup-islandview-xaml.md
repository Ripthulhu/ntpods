---
version: 1
slug: "windows-winui-ntpods-winui-popup-islandview-xaml"
primary_target: "windows/winui/NTPods.WinUI/Popup/IslandView.xaml"
related_targets: ["windows/winui/NTPods.WinUI/Popup/IslandWindow.cs", "windows/winui/NTPods.WinUI/Popup/IslandMotion.cs", "windows/winui/NTPods.WinUI/Popup/TrayMenuView.xaml"]
---

# Battery island

Mode: Operate. Native WinUI popup with LibrePods' Apple appearance,
shown on the primary monitor.

The black capsule reveals the device and event first, then battery readings.
Earbuds show Left, Right and Case; headphones show one centered readout. These
remain visible during listening-mode messages. A charging bolt requires a live
reading. Missing values use an em dash; cached values dim and expose their age
through tooltips and accessible help. Show the shared last-seen line only when
the overall battery data is stale.

Use the existing artwork, native text and theme resources. The island remains
non-activating and frameless. Respect high contrast and Windows reduced motion.
Keep it black in light and dark appearance. Limit width to 400 DIP, shrinking
for the primary monitor's work area; allow height to grow with content and text
scaling. Use DESIGN.md for dimensions and motion timings.
Animate ring changes from the displayed position. Battery reports update the
open island without restarting dismissal or producing repeated notifications.
The tray's Show batteries action opens existing readings without reconnecting.
Animation updates follow WinUI rendering frames while moving. Use a one-shot
timer only for dismissal while idle, and update the native window region only
when its rounded pixel bounds change.
Finish a message blend before showing the latest queued event. New notifications
can reverse dismissal; resizing for content continues from the current height.

Reference: LibrePods rewrite 34076c85, IslandContent.kt and IslandWindow.kt.
Run visual tests separately from the app, without connecting to the daemon.

## Checks

Compare connected, listening-mode, nearby-charging and cached states for earbuds
and headphones. Check light/dark, high contrast, reduced motion, text scaling,
localized labels, zero and missing values, and long names. Center the charging
bolt in each ring. Run the motion and presence tests;
check radio freshness and sleep behavior with hardware.
