---
version: 1
slug: "windows-winui-ntpods-winui-popup-islandview-xaml"
primary_target: "windows/winui/NTPods.WinUI/Popup/IslandView.xaml"
related_targets: ["windows/winui/NTPods.WinUI/Popup/IslandWindow.cs", "windows/winui/NTPods.WinUI/Popup/IslandMotion.cs", "windows/winui/NTPods.WinUI/Popup/TrayMenuView.xaml"]
---

# Battery island

Mode: Operate. Native WinUI popup with LibrePods' Apple appearance,
shown on the primary monitor.

The black capsule clips its final-layout content as one layer. Keep text and
artwork opaque during the fold; never squeeze or reflow them. Measure the final height before
opening a hidden popup so new content is not clipped by the previous size.
Earbuds show Left, Right and Case; headphones show one centered readout. These
remain visible during listening-mode messages. A charging bolt requires a live
reading. Missing values use an em dash; cached values dim and expose their age
through tooltips and accessible help. When all known readings are stale and share
an age, show one last-seen line; otherwise show cached ages under their cells.
Reserve one measured, single-line footer for either form, including when all readings
are current or missing. A recently absent case keeps its percentage current for
90 seconds from its original observation; this display grace never restores charging,
nearby presence or audio ownership. Each cell has a 28 DIP
ring beside a 17 DIP percentage, with its label below. Place the quiet status
caption above the 20 DIP device name.

Use the existing artwork, native text and theme resources. The island remains
non-activating and frameless. Respect high contrast and Windows reduced motion.
Keep it black in light and dark appearance. Limit width to 420 DIP, shrinking
for the primary monitor's work area; allow height to grow from 176 DIP with content and text
scaling. Use DESIGN.md for dimensions and motion timings.
Animate ring changes from the displayed position. Battery reports update the
open island without restarting dismissal or producing repeated notifications.
The tray's Show batteries action opens existing readings without reconnecting.
Animation updates follow WinUI rendering frames while moving. Use a one-shot
timer only for dismissal while idle, and update the native window region only
when its rounded pixel bounds change.
Finish a message push before showing the latest queued event. New notifications
can reverse dismissal; resizing for content continues from the current height.
Reserve a fixed 108 DIP action slot beside the device name. Keep its labels
measured while hidden, and let the slot grow vertically with text scaling.
Reserve the full 100% readout width when choosing the battery-row layout.
A paired nearby
device shows Connect there, then progress or Retry without moving the artwork,
name or battery row. Below 340 DIP use a 48 DIP icon button, preserving its
accessible name. Use audio output readiness for Connected; a control-only
connection still shows Nearby. Clip the 200 ms vertical status push. A hidden
action loses input and accessibility immediately. Keep the island open during
connection progress or automatic-return grace and reset its four-second
dismissal interval when either finishes.
A matching case close freezes nearby or connected battery content and progress
through the fold; listening-mode and other notices stay independent. Hide the
action during normal playback startup and the bounded four-second owned-case
return grace. Repeated lid events must not renew that grace. Pairing success
suppresses the matching opening until a real close/open cycle. With reduced
motion, render each status and action state directly without idle frame work.

Reference: LibrePods rewrite 6a135f4415cfb128fbf5aec7e07be40b953734f8,
IslandContent.kt, IslandWindow.kt and IslandBattery.kt. Keep desktop placement;
Android's camera and status-bar geometry do not apply.
Run visual tests separately from the app, without connecting to the daemon.

## Checks

Compare connected, listening-mode, nearby-charging and cached states for earbuds
and headphones. Check light/dark, high contrast, reduced motion, text scaling,
localized labels, zero and missing values, and long names. Center the charging
bolt in each ring. Run the motion and presence tests;
check radio freshness and sleep behavior with hardware.
