---
name: NTPods WinUI
description: Apple-style AirPods settings with native Windows controls.
colors:
  accent-light: "#0066D6"
  accent-dark: "#0A84FF"
  toggle-light: "#34C759"
  toggle-dark: "#30D158"
  toggle-hover-light: "#30B851"
  toggle-hover-dark: "#34C759"
  toggle-pressed: "#248A3D"
  toggle-knob: "#FFFFFF"
  battery-light: "#248A3D"
  battery-dark: "#30D158"
  page-light: "#F2F2F7"
  page-dark: "#000000"
  group-light: "#FFFFFF"
  group-dark: "#1C1C1E"
  secondary-light: "#636366"
  secondary-dark: "#A1A1A6"
  divider-light: "#D1D1D6"
  divider-dark: "#38383A"
  selected-light: "#FFFFFF"
  selected-dark: "#48484A"
  selected-text-light: "#000000"
  selected-text-dark: "#FFFFFF"
  track-light: "#E3E3E8"
  track-dark: "#1C1C1E"
  hover-light: "#E7E7EB"
  hover-dark: "#2C2C2E"
typography:
  page-title:
    fontSize: "30px"
    fontWeight: 600
  device-title:
    fontSize: "24px"
    fontWeight: 600
  section-title:
    fontSize: "14px"
    fontWeight: 600
  body:
    fontSize: "16px"
  status:
    fontSize: "13px"
  label:
    fontSize: "12px"
rounded:
  group: "24px"
  quiet-button: "18px"
  mode-button: "26px"
  mode-thumb: "27px"
  mode-track: "30px"
spacing:
  tight: "4px"
  compact: "8px"
  related: "12px"
  inset: "16px"
  group: "20px"
  section: "24px"
  settings-section: "28px"
  page-bottom: "48px"
components:
  group-light:
    backgroundColor: "{colors.group-light}"
    rounded: "{rounded.group}"
    padding: "20px"
  group-dark:
    backgroundColor: "{colors.group-dark}"
    rounded: "{rounded.group}"
    padding: "20px"
  setting-row:
    typography: "{typography.body}"
    padding: "16px 20px"
  setting-toggle:
    width: "52px"
  quiet-button:
    backgroundColor: "transparent"
    rounded: "{rounded.quiet-button}"
    padding: "8px 12px"
  listening-mode:
    backgroundColor: "transparent"
    rounded: "{rounded.mode-button}"
    height: "54px"
    padding: "0px"
  listening-track-light:
    backgroundColor: "{colors.track-light}"
    rounded: "{rounded.mode-track}"
    padding: "4px"
  listening-track-dark:
    backgroundColor: "{colors.track-dark}"
    rounded: "{rounded.mode-track}"
    padding: "4px"
---

# Design System: NTPods WinUI

## Overview

NTPods follows LibrePods' Apple appearance: a light gray or black page, rounded inset groups, device artwork, green switches and battery rings. Text, navigation, keyboard controls and accessibility use native Windows behavior.

This record covers the main window, device page and app settings. `App.xaml` supplies the shared resources; the page and control XAML files supply the composition. The tray and island popup have separate layouts. All numeric lengths below are WinUI device-independent pixels (DIP), except the explicitly identified window pixel sizes. The portable frontmatter uses `px` to represent those logical lengths. Opaque XAML ARGB colors are written as equivalent CSS RGB hex values.

## Colors

Green identifies enabled switches and battery readings. Blue is the native control accent. Light appearance uses a darker battery green than the switch fill so the small rings remain distinct against the page.

The `page`, `group`, `track` and `selected` pairs define the surface layers. The `secondary` pair is for descriptions, status and metadata; `divider` separates rows. `hover` supplies expander and checked listening-button pointer states. Selected listening icons use `selected-text`.

Use the matching `Apple*` or `NTPodsAccentBrush` theme resource in XAML. Normal primary text and most standard control states inherit WinUI resources. High contrast replaces custom surface colors with Windows window, text, highlight and highlight-text resources; it does not use either fixed palette. Native controls use the same blue accent through the Windows accent shade resources.

## Typography

Text inherits the WinUI theme font; the app does not bundle or set a custom text family. The intended native family is Segoe UI. Keep font fallback and text rendering under Windows control.

The settings page title uses `page-title`; the centered device name uses `device-title`. `SectionTitle` uses the smaller semibold role in secondary text color. Row labels and every expander heading use `body` at normal weight. Optional help and actionable errors use `status`; listening-mode and battery part labels use `label`. Battery percentages use 17 DIP; the heart-rate reading uses 28 DIP semibold. Line height and character spacing remain native defaults. Settings labels and descriptions wrap, including translations. The device name and connection status stay on one line with ellipsis.

## Layout

Both pages use a full-width vertical scroll area so the side margins accept scrolling and the scrollbar sits at the window's right edge. The centered content column has a 688 DIP maximum outer width and 24 DIP side padding, leaving at most 640 DIP for controls. Device sections have 24 DIP gaps; app settings sections have 28 DIP gaps. The device page starts with 20 DIP top padding; app settings uses 24 DIP. Both end with 48 DIP padding. Current source retains the same side padding as the window narrows; it has no padding breakpoint.

Grouped rows place wrapping text on the left and the control on the right. Repeated feature rows have a 64 DIP minimum height, 20 DIP horizontal padding, 16 DIP vertical padding and a 16 DIP gap before the switch. Standard inset dividers stop 16 DIP short of each side. Expanders use 20 DIP horizontal and 16 DIP vertical header padding; expanded content uses 20 DIP side and bottom padding with 8 DIP at the top.

The battery area has three equal columns, or one centered column for headphones. Each part has proportional artwork, a compact circular gauge, a percentage and a label. The device header's three-dot menu includes app settings; a Back button returns to the device page. The window opens at 820 × 880 physical pixels. Its minimum outer size is a 420 × 540 DIP client area plus the window borders, capped at the monitor's work area.

Keep symmetric 40 DIP columns on either side of the device header, even when only the right side contains an action. The name and status rows reserve at least 32 DIP each. Showing Connect or Repair, or changing a status label, must not move the settings below or shift the device name off center.

## Elevation & Depth

The main groups use solid fills and tonal separation without shadows. The listening selector adds a refracting capsule with a fine directional rim and a soft, downward shadow. Its native fallback uses a solid fill and one-DIP divider-colored outline. Standard menus, tooltips and focus feedback retain their WinUI templates; the tray has its own XAML/acrylic window. The serial-number privacy cover uses a local acrylic brush; it is confined to that field.

## Shapes

Use the `group` radius for cards and expanders. An expanded group keeps rounded outer corners while joining its header and content without an internal corner. Quiet actions use the smaller `quiet-button` radius. Listening controls use the `mode-track`, `mode-thumb` and `mode-button` radii for the outer rail, moving selection and native mode hit targets respectively.

Groups have no outer border. Internal dividers are one DIP high. Battery gauges are 34 DIP circles, and the listening selector's icons fit within 28 DIP squares. `ListeningModeIcon` filters the original monochrome artwork at twice the display resolution; retain its native fallback. Device artwork uses uniform scaling without stretching.

## Components

- **Inset groups and expanders.** Reuse `Card`, `GroupExpander`, `SectionTitle` and `InsetDivider`. A card normally owns its padding; a group of rows sets card padding to zero so each row and divider controls its inset. Equalizer, device adjustments and additional sensor controls use native expanders. Keep one rounded outer surface throughout expansion; the header must not abruptly change from rounded to square. Reveal content at its natural size through an animated viewport, with nearby cards moving alongside it. Open over 240 ms and close over 180 ms, continuing from the current height when interrupted. Disable closing content's interaction immediately, keep native expand/collapse events and keyboard behavior, and snap to the final layout when Windows animations are off.
- **Switch rows.** `SettingToggle` retains `DefaultToggleSwitchStyle`, aligns right, and removes the redundant on/off captions. Labels and accessible names come from the same localization resource. On, hover and pressed fills use the toggle color tokens. Routine confirmation, pending and unavailable explanations belong in tooltips and accessibility help, not repeated lines beneath every label.
- **Unreported preferences.** Known supported preferences can accept an explicit choice without a current report. Use an unselected native On/Off chooser for unknown booleans, reserving the same 144 DIP control slot as the reported switch. Show sent choices separately from confirmed reports in tooltips and accessibility help. Require both hold actions before Apply; never guess the other side. Saved choices belong to their pair and never send automatically when loaded.
- **Hearing edits.** Require explicit Apply for audiogram changes; debounce tuning gestures and preserve untouched settings from the current pair's confirmed profile.
- **Hardware checks.** Show saved per-ear results in a collapsed expander with manual Refresh. Keep device timestamps distinct from the time read, and use neutral status text: cached flags are not a current health verdict.
- **Listening modes.** One draggable capsule marks the current mode. The rail contains only the modes supported by the model. Legacy ANC models offer Off directly; newer Pro models use the reported or saved Allow Off choice. Adaptive expands to three of five equal units within the same rail; Off is hidden during that expansion. Strength runs from 0 to 100 across the Adaptive region. The labels below the rail keep stable wrapping widths and move with the segment centers. Mode animations must not change the label row height or push other sections down. Disabled and disconnected states keep the same rail with dimmed icons; native button backgrounds must not create separate pills. Do not display a selected mode when its value is unknown.
- **Listening interaction.** The capsule follows the pointer continuously across Adaptive strength. Keep the full 0–100 range even when firmware reports rounded values. Coalesce live changes while dragging and send the final position on release, waiting for mode confirmation before sending strength. Requested values remain separate from device readback without persistent status copy. Native toggle buttons provide keyboard mode selection; a native slider provides keyboard and automation access to strength. Its focused state draws a two-DIP outline around the Adaptive region. A send in progress must not interrupt a gesture.
- **Listening material.** Follow the Apple selector in the pinned LibrePods rewrite: a transparent refracting capsule, a fine directional rim and a soft shadow. Its lens bends the rail beneath it; the selected icon stays sharp above the lens. Adaptive endpoint markers fade as the capsule covers them, including in the refracted source, so they do not collide with the icon or create colored fragments at the rim. Adaptive adds a translucent inset region rather than a solid selected button. Preserve the native opaque selection in high contrast and when the graphics surface is unavailable. Keep the existing desktop dimensions and let the glass and shadow extend beyond the rail during a gesture.
- **Listening motion.** Keep all selection icons loaded and crossfade them only when the mode changes. Pointer movement changes a retained translation without easing or reloading artwork. Keep the latest drag position and apply visual updates once per rendering callback, avoiding repeated layout work from high-rate mouse input. Mode widths, released positions and press expansion use LibrePods' critically damped springs (stiffness 1500, unit mass). Drag velocity stretches the capsule horizontally; release returns that stretch with damping 0.2. Interruptions retain the current position and velocity. Scale the artwork without changing the thumb's input bounds, and use the same interpolated geometry for drawing and hit testing. Reuse glass effects and their source links while dimensions remain unchanged. Drive active frames with a high-resolution clock and stop rendering when settled. Animation frames only draw; they never send device commands. Windows' animation preference disables spring motion and icon fades.
- **Battery readouts.** Display left, right and case readings independently, or one centered reading for headphones. Preserve a known percentage and its original age when a new report omits it; use an em dash when no reading is known. Tie cached data to the device identity. Dim stale readings independently of artwork: a connected, available bud stays bright, while a bud explicitly reported unavailable dims even if its partner is connected. Show the centered charging bolt only with a live reading. These controls send no device commands.
- **Artwork.** Reuse the closest bundled artwork when an exact model image is unavailable, without changing its capabilities or displayed name. Retain known artwork during nearby charging and missing model reports. Decode replacement images before assigning them; a slow or failed load must not blank the image. Visibility fades take 180 ms to show and 120 ms to hide. Keep the old bitmap through the hide animation and ignore obsolete load completions. Respect reduced motion.
- **Inputs and actions.** Keep native `TextBox`, `ComboBox`, `RadioButtons`, `Slider`, `Button`, `MenuFlyout` and `InfoBar` behavior. `QuietButton` is the transparent action variant. The device actions button uses it with a 40 DIP square target. The rename field, radio choices, application language field and ordinary sliders use their native control templates.
- **Microphone settings.** Use a collapsed `GroupExpander` titled Microphone for the enable switch, behavior, stop delay, manual controls and keyboard shortcut. Keep active capture, recovery and error status in its header so those states remain visible while the controls are closed. Opening or closing the section must not change microphone settings or stop capture. Capture a shortcut in a native dialog; preserve the previous binding on cancellation or conflict. The global shortcut starts or stops capture in Manual mode. Show confirmed microphone changes in the existing Dynamic Island, with a microphone icon and a slash when off. Announce readiness only after decoded audio arrives.
- **Navigation and status.** The device header's three-dot menu combines device actions and app settings. Settings stays accessible while the daemon is unavailable; Back returns focus to the menu button. Keep the space above the device name free of duplicate device tabs or a separate Settings row. The device header owns connection status; battery readouts own charge information. Show actionable errors once, keep active microphone and recovery information visible, and omit routine success messages.
- **Pairing.** Keep Pair AirPods in the header menu and empty state, available without the daemon. Opening a nearby unpaired case can show the bottom connection card before its pairing gesture, including while NTPods is in the tray. Keep this card white with dark text and a blue Connect button in both light and dark appearance; high contrast uses Windows system colors. Use the advertised model or discovered name, bundled model artwork, Connect and close. Slide the whole rounded card from the bottom; respect reduced motion. Automatic appearance must not activate the window; explicit clicks retain native keyboard behavior. Show the model-specific pairing gesture on the first lid card, before Connect; Connect starts discovery for a single matching AirPods endpoint. Keep device lists out of this card. If several matching AirPods are available, wait and ask the user to close the other cases. The advertised model selects the title, artwork and instructions; Windows discovery supplies the endpoint. An invitation already tied to a Windows endpoint can pair it directly. Allow 60 seconds for discovery and pairing; transient pairing failures keep the instructions visible. After pairing, retain the same card with a visible Paired state and fresh battery readings from the paired device until Continue or close. Unknown and stale readings stay unknown. Closing the case cancels and dismisses its lid invitation; reopening can show it again, including during dismissal. Keep unrelated Classic and reconnect cards independent. Suppress duplicate lid invitations and repeated Classic invitations, and pause expiry during interaction, pairing or the Paired state. Keep manual selection for renamed devices. Pause background Classic discovery while a card or dialog is open; allow that card's requested scan. Inhibit discovery during microphone capture and shutdown.
- **Tray menu.** Preserve the custom XAML/acrylic window, shared listening icons and model-based mode availability. Open on the cursor's monitor, fit within its work area and scroll vertically when needed. Unlike the island, this menu takes focus for keyboard input and closes on Escape or deactivation.
- **Window and page transitions.** On opening, the content scales from 0.985 to 1 over 220 ms and fades from 0.88 to 1 over 140 ms. Device and app settings crossfade over 160 ms; only the active page accepts input. Interrupted page fades continue from their current opacity. Finish immediately when Windows animations are disabled.
- **Device and section transitions.** Reuse `SectionTransitions` when a device identity or feature visibility changes. Move remaining sections and reveal new ones over 180 ms; hide removed sections over 120 ms. Normal snapshots and expander frames must not restart these transitions or force another layout pass. Each pair retains its own readings and capabilities. Finish immediately with Windows animations disabled.
- **Scrollbars.** Preserve native scrollbar input and accessibility. Reveal immediately while scrolling or interacting with the bar, then fade over 120 ms after 400 ms idle. Hovering other page content must not keep the bar visible. Disable the fade when Windows animations are off.
- **Dynamic Island.** Keep the frameless black capsule on the primary monitor, 16 DIP below the work-area top, without taking focus. It stays black in light and dark appearance; high contrast uses Windows system colors. The surface is at most 400 DIP wide, shrinks to fit the work area, and grows above its 160 DIP minimum height to fit content. Corners are 36 DIP. Device artwork and event text sit above three independent battery readouts for earbuds, or one centered readout for headphones. Charging bolts require live readings. Missing values use an em dash; cached values dim and expose their age through tooltips and accessible help. Show a shared last-seen line when the overall battery data is stale. Reserve a 108 DIP action slot beside the name in every header so Connect, progress, Retry and message icons do not shift the artwork or text. The island says Connected when the selected audio output is ready; a control connection alone still leaves it Nearby.
- **Island motion.** Open over 420 ms without overshoot, revealing the header before the battery row. Fold the final-layout payload as one unit inside the capsule. Measure a hidden island at its new height before opening, so taller content is not clipped. Hold for four seconds, then retract over 220 ms. Changes to the measured content height settle over 180 ms; ring values settle over 280 ms, status text moves through a clipped 180 ms blend, and the fixed Connect area blends labels and progress over 140 ms. Finish the current message blend before showing the latest queued event. Keep the island open during a connection request or automatic-return grace period, then give its result a fresh four-second reading interval. A matching case close freezes either nearby or connected battery content while it folds away; listening-mode and other notices remain independent. New events can reverse dismissal from its current size; ordinary battery updates must not restart dismissal. Render only while moving and update the native window region only when its pixel bounds change. Respect reduced motion, including direct action and label changes. The tray's Show batteries action uses existing readings without reconnecting audio.

Use [AGENTS.md](AGENTS.md) for change and review requirements, and
[PRODUCT.md](PRODUCT.md) for platform and feature constraints.

## Do's and Don'ts

- Reuse the native controls, shared theme resources and motion helpers. Preserve keyboard focus, accessible names, high contrast and reduced motion.
- Keep routine status in the control state, tooltip or accessible help. Show actionable errors once and keep active microphone or recovery state visible.
- Keep settings centered, side margins scrollable and the scrollbar at the window edge. State changes must not shift stable content.
- Check long translated labels, different models, missing readings and interrupted animations alongside light, dark and narrow layouts.
- Use the committed images in `docs/images` as appearance references, checking them against the current source before replacing them. Keep README screenshots prominent and the video secondary. Capture sharp images at high resolution and videos with 60 distinct frames per second; leave space between the app and island and remove capture-tool highlights. Preview data stays isolated from real device commands.
