---
version: 1
slug: "windows-winui-ntpods-winui-popup-islandview-xaml"
primary_target: "windows/winui/NTPods.WinUI/Popup/IslandView.xaml"
related_targets: ["windows/winui/NTPods.WinUI/Popup/IslandWindow.cs", "windows/winui/NTPods.WinUI/Popup/IslandMotion.cs", "windows/winui/NTPods.WinUI/Popup/TrayMenuView.xaml"]
---

# Battery island

Mode: Operate. Native WinUI popup with LibrePods' Apple appearance,
shown on the primary monitor.

The black capsule folds its final-layout content as one layer. It reveals the
device and event first, then battery readings. Measure the final height before
opening a hidden popup so new content is not clipped by the previous size.
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
Reserve a fixed 108 DIP action slot beside the device name. A paired nearby
device shows Connect there, then progress or Retry without moving the artwork,
name or battery row. Use audio output readiness for Connected; a control-only
connection still shows Nearby. Clip the short vertical status blend. A hidden
action loses input and accessibility immediately. Keep the island open during
connection progress or automatic-return grace and reset its four-second
dismissal interval when either finishes.
A matching case close freezes nearby or connected battery content through the
fold; listening-mode and other notices stay independent. With reduced
motion, render each status and action state directly.

Reference: LibrePods rewrite 62ef3358, IslandContent.kt and IslandWindow.kt.
Run visual tests separately from the app, without connecting to the daemon.

## Checks

Compare connected, listening-mode, nearby-charging and cached states for earbuds
and headphones. Check light/dark, high contrast, reduced motion, text scaling,
localized labels, zero and missing values, and long names. Center the charging
bolt in each ring. Run the motion and presence tests;
check radio freshness and sleep behavior with hardware.
