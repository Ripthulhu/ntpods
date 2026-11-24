---
name: NTPods WinUI
description: Apple-style AirPods settings with native Windows controls.
colors:
  accent-light: "#0088FF"
  accent-dark: "#0091FF"
  toggle-light: "#34C759"
  toggle-dark: "#34C759"
  toggle-hover-light: "#30B851"
  toggle-hover-dark: "#34C759"
  toggle-pressed: "#248A3D"
  toggle-knob: "#FFFFFF"
  battery-light: "#35C759"
  battery-dark: "#2ED158"
  page-light: "#F2F2F7"
  page-dark: "#000000"
  group-light: "#FFFFFF"
  group-dark: "#1C1C1E"
  secondary-light: "#636366"
  secondary-dark: "#A1A1A6"
  divider-light: "#88888840"
  divider-dark: "#88888840"
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
    fontSize: "22px"
    fontWeight: 800
  device-title:
    fontSize: "22px"
    fontWeight: 800
  section-title:
    fontSize: "14px"
    fontWeight: 600
  body:
    fontSize: "14px"
  status:
    fontSize: "12px"
  label:
    fontSize: "12px"
rounded:
  group: "28px"
  quiet-button: "18px"
  mode-button: "26px"
  mode-thumb: "27px"
  mode-track: "30px"
spacing:
  tight: "4px"
  compact: "8px"
  related: "12px"
  inset: "16px"
  group: "16px"
  section: "16px"
  settings-section: "16px"
  page-bottom: "32px"
components:
  group-light:
    backgroundColor: "{colors.group-light}"
    rounded: "{rounded.group}"
    padding: "16px"
  group-dark:
    backgroundColor: "{colors.group-dark}"
    rounded: "{rounded.group}"
    padding: "16px"
  setting-row:
    typography: "{typography.body}"
    padding: "15px 16px"
  setting-toggle:
    width: "64px"
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

The device screens follow LibrePods' Apple screens and shared primitives: `AppleSettingsScreen`, `StyledScaffold`, `StyledList`, `StyledListItem`, `StyledSwitch` and `BatteryView`. Their filled inset groups, compact artwork, centered titles and typography are the visual authority. WinUI retains native input, focus, keyboard and automation behavior.

This record covers the main window, device page and app settings. `App.xaml` supplies the shared resources; the page and control XAML files supply the composition. The tray and island popup have separate layouts. All numeric lengths below are WinUI device-independent pixels (DIP), except the explicitly identified window pixel sizes. The portable frontmatter uses `px` to represent those logical lengths. Opaque XAML ARGB colors are written as equivalent CSS RGB hex values.

## Colors

Enabled switches use #34C759 in both appearances. Battery rings use #35C759 in light appearance and #2ED158 in dark appearance. Controls use the Apple source accents #0088FF and #0091FF.

The `page`, `group`, `track` and `selected` pairs define the surface layers. The `secondary` pair is for descriptions, status and metadata; `divider` separates rows. `hover` supplies expander and checked listening-button pointer states. Selected listening icons use `selected-text`.

Use the matching `Apple*` or `NTPodsAccentBrush` theme resource in XAML. Normal primary text and most standard control states inherit WinUI resources. High contrast replaces custom surface colors with Windows window, text, highlight and highlight-text resources; it does not use either fixed palette. Native controls use the same blue accent through the Windows accent shade resources.

## Typography

The app bundles the same Roboto Flex font as LibrePods. `AppleBodyFont` and the native content font resource use that file. Body text uses the 400 weight; centered scaffold titles use the 800 weight at 22 DIP. Windows provides shaping and fallback for missing glyphs. Keep the font's OFL notice with the payload.

Row labels and expander headings use 14 DIP at normal weight. Section captions use the 14-DIP semibold secondary role; helper copy, connection status, slider captions and battery readings use 12 DIP. Settings labels and descriptions wrap. Device names and scaffold titles use one line with ellipsis; the Name row retains the actual value separately.

## Layout

Keep the AirPods Dongle card below the shared settings. It stays reachable before
a pair is selected, when the device settings are hidden. The device header
reconnects the saved pair through the selected dongle.
Spatial audio uses the shared navigation row and a single inset action group.
When a bud needs calibration, name that side in the room’s playback status.
In the listening room, highlight each selected speaker. Ctrl-click toggles
individual channels, and the native speaker picker exposes the same selection.
Keep firmware counters behind the experimental settings preference.

Use native acrylic for the round Back and Settings buttons, the header Connect action
and the phone setup action. A one-DIP directional rim catches light along the
upper edge. The two dongle pairing actions share one material, as do the Phone
and This PC handoff actions. Keep their hit targets independent. A native
connection with device actions uses More in the Settings slot. The dongle
shows the pair name once in the header. App Settings puts appearance and local
preferences before phone linking.
A 16-DIP fade at the top of each scrolling page softens content passing the
fixed header. High contrast removes the fade and uses system colors for glass
surfaces. Windows selects the solid material fallback when transparency is off.
Settings groups retain their filled backgrounds. Switch thumbs use acrylic
while pressed and sliders retain their existing lens behavior.
[Apple materials guidance](https://developer.apple.com/design/human-interface-guidelines/materials)


The device and app settings screens use a centered column with a maximum outer width of 608 DIP and 16-DIP side padding. Sections use 16-DIP gaps. The scroll area spans the window, including side margins, with its scrollbar at the edge. Body padding is 16 DIP above and 32 DIP below. These insets remain stable as the window narrows.

The scaffold has a centered 22-DIP title and 48-DIP round navigation or action buttons in symmetric side slots. Subpage bars are 64 DIP high. The main header reserves a 44-DIP connection-status row so showing Connect or Repair keeps the title and settings in place.

Navigation rows use filled 28-DIP groups, a 58-DIP minimum height and 16-DIP horizontal insets. Name shows its current value before the chevron. Hearing Health and Press and Hold each share one filled group with inset dividers. Standalone navigation rows use the same fill and radius. Text and trailing values remain vertically centered; long labels can grow the row.

The battery hero is capped at 350 DIP. Bud artwork shares the left column and the case occupies the right, separated by 16 DIP. Independent left and right gauges sit together below the buds; the case gauge sits below the case. Keep every reported percentage and freshness independent, including near-equal readings. Individual artwork wrappers preserve per-ear availability dimming. Headphones span the hero with one reading. Do not add a second large device title or put Windows companion tools between the hero and Name.

Filled groups have 16-DIP content insets, 28-DIP corners and no outer border. Repeated switch rows use a 58-DIP minimum height and a 12-DIP text/control gap. Switch tracks are 64 by 28 DIP with 39 by 24-DIP white knobs. Ordinary sliders use a six-DIP track and a white pill thumb; the listening-mode rail retains its separate glass control and motion. Standard inset dividers stop 16 DIP short of each side. Expanders use 16-DIP side padding and a 16-DIP header inset.

The window opens at 820 × 880 physical pixels. Its minimum outer size remains a 420 × 540 DIP client area plus window borders, capped at the monitor's work area.

## Device navigation

Use the same Apple section names, order and destinations as LibrePods. Immediately after the battery hero, show Name, Hearing Health, Listening Mode, Heart Rate, Press and Hold AirPods (Left and Right), Call Controls, Audio, Automatic Ear Detection, Microphone Mode, Pause media when falling asleep, Optimized Charge Limit, Accessibility, Off Listening Mode and About. Omit unsupported sections. Do not substitute head tracking for Head Gestures or label microphone controls Recorder when recording is unavailable.

Open settings in real subpages with a round Back action and centered scaffold title. Hearing Health opens Hearing Protection or Hearing Aid. Audio keeps Personalized Volume and Conversation Awareness together and opens Equalizer. Accessibility contains Press Speed, Press and Hold Duration, Listening Mode options, Headphone Accommodations, Tone Volume, Volume Control and Volume Swipe Speed. Headphone Accommodations opens Transparency. Keep Listen on, shared companion settings, Windows volume, playback, microphone, sensors, nearby features and hardware checks under the NTPods section after the common device sections. Reuse existing controls and their pending, identity, cleanup and command behavior; navigation must not duplicate them or send settings.

Equalizer's Recommended sound or Custom EQ label follows the displayed live choice, including while a write is pending. Keep confirmation status in the existing accessibility help and tooltip. Do not show the old device preset beside newly edited bands. Explanatory and value text uses the shared 12-DIP helper size.

Headphone Accommodations follows the shared Apple screen: master toggle, Audio tuning choices, independent Phone and Media switch rows and Transparency. Put Audiogram first, followed by Balanced Tone, Vocal Range and Brightness. Show the three fixed Optimization stops (Slight, Moderate, Strong) only for stock tunings. Audiogram uses the hearing profile saved on the selected AirPods and requires a complete validated source; it never rewrites that profile. Display received read-only flags and recognized selections while keeping editing disabled. Unmatched curves remain Custom settings, with no guessed Audiogram classification or Optimization value. Do not add Customize Audio or Play Sample until their decision flow and assets are verified. Send changes live and preserve unedited fields. Keep a successfully sent choice visible while awaiting a device report, without a routine warning or an automatic reconnect. Store requested choices separately from received values; fresh reports supersede them, and connection changes discard them. Remember Phone/Media choices per pair while the master is off; a fresh session must not reset them.

Transparency has an independent master group and five separate slider groups in order: Amplification, Balance, Tone, Ambient Noise Reduction and Own Voice Amplification, followed by a separate Conversation Boost group. The subpage has no redundant expander. Use the source-backed PersonalAudio conversion described in the protocol reference. Noncanonical per-ear gains remain visible but disable the combined gain controls; other adjustments preserve them. Preserve separate per-ear values and both EQ curves until the user edits the corresponding control. Read automatically on connection and send valid changes live, with one write in flight and the latest gesture retained until it can be sent. Transparency and Optimization share the inset glass slider style. Their slider groups use 24-DIP horizontal and 16-DIP vertical padding, with an eight-DIP caption-to-track gap. The enlarged pressed thumb stays within the horizontal inset. Its 40-by-24-DIP white resting thumb reveals a refracting lens, directional rim and restrained shadow during a press, using the existing AndroidLiquidGlass lens math and native rendering. Keep its keyboard and RangeValue automation exposed; availability gates each input directly, with no focusable content wrapper. See [the protocol reference](docs/WINDOWS-AIRPODS-CONTROL-PROTOCOL.md) for verified layouts and limits.

## Phone connection and selected route

Phone connection is always available in App Settings and under NTPods on the
device page. Keep system Bluetooth pairing and app linking distinct. Setup opens
Windows Bluetooth settings, refreshes bonded phones on return, then creates a
linking code only after an explicit action for the selected phone and AirPods.
Explain that the phone uses the PC's Bluetooth adapter while AirPods may stay on
the USB dongle. Match LibrePods' PC connection terminology and Apple groups.
Show waiting separately from an authenticated connection. Opening or rendering
setup must never replace an existing link or change call consent. Clear temporary
codes on dismissal, selection or session changes; exclude copied codes from
clipboard history and roaming. Keep transfer controls in the existing Listen on card.

The header and battery hero follow the selected native or dongle route. Retain
each component's freshness and artwork identity. Header Connect targets that
route explicitly. Keep dongle pairing, Bluetooth audio input and diagnostics in
one collapsed dongle group; reveal its picker when selection needs attention.
Do not repeat ordinary battery, listening-mode or reconnect controls there.

## Elevation & Depth

The main groups use solid fills and tonal separation without shadows. The listening selector adds a refracting capsule with a fine directional rim and a soft, downward shadow. Its native fallback uses a solid fill and one-DIP divider-colored outline. Standard menus, tooltips and focus feedback retain their WinUI templates; the tray has its own XAML/acrylic window. The serial-number privacy cover uses a local acrylic brush; it is confined to that field.

## Shapes

Use the `group` radius for cards and expanders. An expanded group keeps rounded outer corners while joining its header and content without an internal corner. Quiet actions use the smaller `quiet-button` radius. Listening controls use the `mode-track`, `mode-thumb` and `mode-button` radii for the outer rail, moving selection and native mode hit targets respectively.

Groups have no outer border. Internal dividers are one DIP high. Battery gauges are 34 DIP circles, and the listening selector's icons fit within 28 DIP squares. `ListeningModeIcon` filters the original monochrome artwork at twice the display resolution; retain its native fallback. Device artwork uses uniform scaling without stretching.

## Components

- **Inset groups and expanders.** Reuse `Card`, `GroupExpander`, `SectionTitle` and `InsetDivider`. A card normally owns its padding; a group of rows sets card padding to zero so each row and divider controls its inset. Equalizer, device adjustments and additional sensor controls use native expanders. Keep one rounded outer surface throughout expansion; the header must not abruptly change from rounded to square. Reveal content at its natural size through an animated viewport, with nearby cards moving alongside it. Open over 240 ms and close over 180 ms, continuing from the current height when interrupted. Disable closing content's interaction immediately, keep native expand/collapse events and keyboard behavior, and snap to the final layout when Windows animations are off.
- **Switch rows.** `SettingToggle` retains `DefaultToggleSwitchStyle`, aligns right, and removes the redundant on/off captions. Labels and accessible names come from the same localization resource. On, hover and pressed fills use the toggle color tokens. Routine confirmation, pending and unavailable explanations belong in tooltips and accessibility help, not repeated lines beneath every label.
- **Listen on.** Keep this Windows companion group below the common device sections. Place the Phone and This PC actions together inside one `GlassActionGroup`. Use equal-width `QuietButton` controls with native icons and wrapping labels. Reserve a 20-DIP progress slot beside the title during a transfer. Keep routine guidance in tooltips and accessible help and show a failed transfer once below the actions.
- **Unreported preferences.** Known supported preferences can accept an explicit choice without a current report. Automatic ear detection always uses the standard switch, including saved choices. An unknown charging-case sounds setting uses an unselected native On/Off chooser in the same 144 DIP control slot as the reported switch. Show sent choices separately from confirmed reports in tooltips and accessibility help. Send paired hold actions only after both choices are valid; never guess the other side. Saved choices belong to their pair and never send automatically when loaded.
- **Hearing edits.** Commit complete validated audiogram edits automatically; debounce tuning gestures and preserve untouched settings from the current pair's confirmed profile.
- **Hearing profile presentation.** Load the validated stored profile automatically. Do not display a blanket missing-audiogram or Experimental warning above a loaded fitting. Keep missing-profile and failed-write handling tied to the actual state, with editing and enrollment safeguards enforced by the backend.
- **USB hearing controls.** Reuse Hearing Aid, Headphone Accommodations and Hardware checks through their device subpages. A connected PC Bluetooth session keeps its controls; otherwise the dongle supplies these two groups with its own reported model and session. Changing the USB target clears drafts and confirmations. Preserve the experimental opt-in and model gates.
- **Dongle selection.** Keep the native board picker in the existing dongle group when several boards are attached or the selected board is missing. Follow the hardware serial across USB personality changes and never silently select a different board. Keep AirPods connection separate from USB audio availability; control-only attachment retains the group and its supported actions. Codec/profile edits require idle audio, while Game Mode remains a live firmware control. Routine availability belongs in accessible help; do not claim route ownership from a connected link.
- **Hardware checks.** Load saved per-ear results and device metadata automatically on connection. Keep results in a collapsed expander and raw data in secondary details. Keep device timestamps distinct from the time read; cached flags are not a current health verdict. An in-case check requires explicit confirmation and verified support. Show the next physical action and separate new-result progress for each ear. Label cancellation Stop waiting because it does not stop the firmware check.
- **Find nearby.** Use a collapsed native expander with explicit Start and Stop. Show only the selected pair's verified Bluetooth signal, its trend and the age of the last report. Keep the last signal dimmed when reports stop; do not infer distance, direction or a lost device. Pause live guidance during microphone use. Stop this view's search when it collapses, unloads, leaves the device page, changes pairs, loses the daemon or hides with the window. Opening the card never starts a search or connects audio.
- **Listening modes.** One draggable capsule marks the current mode. The rail contains only the modes supported by the model. Legacy ANC models offer Off directly; newer Pro models use the reported or saved Allow Off choice. Adaptive expands to three of five equal units within the same rail; Off is hidden during that expansion. Strength runs from 0 to 100 across the Adaptive region. The labels below the rail keep stable wrapping widths and move with the segment centers. Mode animations must not change the label row height or push other sections down. Disabled and disconnected states keep the same rail with dimmed icons; native button backgrounds must not create separate pills. Do not display a selected mode when its value is unknown.
- **Listening interaction.** The capsule follows the pointer continuously across Adaptive strength. Keep the full 0–100 range even when firmware reports rounded values. Coalesce live changes while dragging and send the final position on release, waiting for mode confirmation before sending strength. Requested values remain separate from device readback without persistent status copy. Native toggle buttons provide keyboard mode selection; a native slider provides keyboard and automation access to strength. Its focused state draws a two-DIP outline around the Adaptive region. A send in progress must not interrupt a gesture.
- **Listening material.** Follow the Apple selector in the pinned LibrePods rewrite: a transparent refracting capsule, a fine directional rim and a soft shadow. Its lens bends the rail beneath it; the selected icon stays sharp above the lens. Adaptive endpoint markers fade as the capsule covers them, including in the refracted source, so they do not collide with the icon or create colored fragments at the rim. Adaptive adds a translucent inset region rather than a solid selected button. Preserve the native opaque selection in high contrast and when the graphics surface is unavailable. Keep the existing desktop dimensions and let the glass and shadow extend beyond the rail during a gesture.
- **Listening motion.** Keep all selection icons loaded and crossfade them only when the mode changes. Pointer movement changes a retained translation without easing or reloading artwork. Keep the latest drag position and apply visual updates once per rendering callback, avoiding repeated layout work from high-rate mouse input. Mode widths, released positions and press expansion use LibrePods' critically damped springs (stiffness 1500, unit mass). Drag velocity stretches the capsule horizontally; release returns that stretch with damping 0.2. Interruptions retain the current position and velocity. Scale the artwork without changing the thumb's input bounds, and use the same interpolated geometry for drawing and hit testing. Reuse glass effects and their source links while dimensions remain unchanged. Drive active frames with a high-resolution clock and stop rendering when settled. Animation frames only draw; they never send device commands. Windows' animation preference disables spring motion and icon fades.
- **Battery readouts.** Display left, right and case readings independently, or one centered reading for headphones. Preserve a known percentage and its original age when a new report omits it; use an em dash when no reading is known. Tie cached data to the device identity. Dim stale readings independently of artwork: a connected, available bud stays bright, while a bud explicitly reported unavailable dims even if its partner is connected. USB battery text uses the secondary theme brush at full opacity when stale, retaining readable contrast and Windows high-contrast colors. Show the centered charging bolt only with a live reading. These controls send no device commands.
- **Artwork.** Reuse the closest bundled artwork when an exact model image is unavailable, without changing its capabilities or displayed name. Retain known artwork during nearby charging and missing model reports. Decode replacement images before assigning them; a slow or failed load must not blank the image. Visibility fades take 180 ms to show and 120 ms to hide. Keep the old bitmap through the hide animation and ignore obsolete load completions. Respect reduced motion.
- **Inputs and actions.** Keep native `TextBox`, `ComboBox`, `RadioButtons`, `Slider`, `Button`, `MenuFlyout` and `InfoBar` behavior. `QuietButton` is the transparent action variant. The device actions button uses it with a 40 DIP square target. The rename field, radio choices, application language field and ordinary sliders use their native control templates.
- **Microphone settings.** Use a collapsed `GroupExpander` titled Microphone for the enable switch, behavior, stop delay, manual controls and keyboard shortcut. Keep active capture, recovery and error status in its header so those states remain visible while the controls are closed. Opening or closing the section must not change microphone settings or stop capture. Capture a shortcut in a native dialog; preserve the previous binding on cancellation or conflict. The global shortcut starts or stops capture in Manual mode. Show confirmed microphone changes in the existing Dynamic Island, with a microphone icon and a slash when off. Announce readiness only after decoded audio arrives.
- **Navigation and status.** The device header's three-dot menu combines device actions and app settings. Settings stays accessible while the daemon is unavailable; Back returns focus to the menu button. Keep the space above the device name free of duplicate device tabs or a separate Settings row. The device header owns connection status; battery readouts own charge information. Show actionable errors once, keep active microphone and recovery information visible, and omit routine success messages.
- **Pairing.** Keep Pair AirPods in the header menu and empty state, available without the daemon. Opening a nearby unpaired case can show the bottom connection card before its pairing gesture, including while NTPods is in the tray. Keep this card white with dark text and a blue-text Connect action in both light and dark appearance; use a quiet gray Continue action after pairing. High contrast uses Windows system colors. Use the advertised model or discovered name, bundled model artwork, Connect and close. Slide the whole rounded card from the bottom; respect reduced motion. Automatic appearance must not activate the window; explicit clicks retain native keyboard behavior. Show the model-specific pairing gesture on the first lid card, before Connect; Connect starts discovery for a single matching AirPods endpoint. Keep the model name as the nearby and paired heading; show the setup gesture as the heading while waiting, then Connecting after selecting the endpoint. Keep device lists out of this card. If several matching AirPods are available, wait and ask the user to close the other cases. The advertised model selects the title, artwork and instructions; Windows discovery supplies the endpoint. An invitation already tied to a Windows endpoint can pair it directly. Allow 60 seconds for discovery and pairing; transient pairing failures keep the instructions visible. After pairing, retain the same card with a visible Paired state and fresh, identity-verified battery readings from that device, observed after this Connect attempt began, until Continue or close. Unknown and stale readings stay unknown. Closing the case cancels and dismisses its lid invitation; reopening can show it again, including during dismissal. Keep unrelated Classic and reconnect cards independent. Suppress duplicate lid invitations and repeated Classic invitations, and pause expiry during interaction, pairing or the Paired state. Keep manual selection for renamed devices. Pause background Classic discovery while a card or dialog is open; allow that card's requested scan. Inhibit discovery during microphone capture and shutdown.
- **Tray menu.** Preserve the custom XAML/acrylic window, shared listening icons and model-based mode availability. Open on the cursor's monitor, fit within its work area and scroll vertically when needed. Unlike the island, this menu takes focus for keyboard input and closes on Escape or deactivation.
- **Pairing artwork and content.** Reuse LibrePods' silent Pro-style loop for known Pro models. Show its decoded matching poster from the first visible frame, keeping it over the video until the card entrance finishes and playback advances, then fade it away over 120 ms. Repeated updates for the same model retain the player. Preserve the source frame rate and aspect ratio in a fitted viewport, without player letterboxing or still artwork showing behind it. Keep a model-identified lid invitation open until the case closes or the user dismisses it. Other models, reduced motion, high contrast and playback failures use the model's still artwork. Keep the player independent of Windows media controls, pause at dismissal and release after the card is hidden. Instructions, progress and paired content crossfade over 350 ms; repeated identical states do not restart motion. Reserve the maximum phase-heading height and retain the measured body height for each invitation, including with larger text, so shorter progress or success content cannot shrink the card. Queue only the latest status text during a text fade, keep outgoing content out of input and accessibility, and finish transitions immediately when hidden or animations are off. Preserve the card's separate outer slide.
- **Window and page transitions.** On opening, the content scales from 0.985 to 1 over 220 ms and fades from 0.88 to 1 over 140 ms. Device and app settings crossfade over 160 ms; only the active page accepts input. Interrupted page fades continue from their current opacity. Finish immediately when Windows animations are disabled.
- **Device and section transitions.** Reuse `SectionTransitions` when a device identity or feature visibility changes. Move remaining sections and reveal new ones over 180 ms; hide removed sections over 120 ms. Normal snapshots and expander frames must not restart these transitions or force another layout pass. Each pair retains its own readings and capabilities. Finish immediately with Windows animations disabled.
- **Scrollbars.** Preserve native scrollbar input and accessibility. Reveal immediately while scrolling or interacting with the bar, then fade over 120 ms after 400 ms idle. Hovering other page content must not keep the bar visible. Disable the fade when Windows animations are off.
- **Dynamic Island.** Keep the frameless black capsule on the primary monitor, 16 DIP below the work-area top, without taking focus. It stays black in light and dark appearance; high contrast uses Windows system colors. The surface is at most 420 DIP wide, shrinks to fit the work area, and grows above its 176 DIP minimum height to fit content. Corners are 36 DIP. A quiet status caption sits above the device name beside proportional artwork. Below it, each battery cell has a 28 DIP ring beside its percentage and a centered label underneath. Headphones use one centered cell. Charging bolts require live readings. A recently absent case keeps its percentage current for 90 seconds from the original observation; this display grace never restores charging, nearby presence or audio ownership. Missing values use an em dash; cached readings dim and retain their own age. When all known readings are stale and share an age, show one last-seen line; otherwise show cached ages beneath their cells. Reserve one measured, single-line freshness footer in every state, and reserve the full 100% readout width when choosing horizontal or stacked battery rows. Keep hidden action labels measured and allow their slot to grow with text scaling. Reserve a 108 DIP action slot beside the name so Connect, progress, Retry and message icons do not shift the artwork or text. Below 340 DIP, use a 48 DIP icon button with an accessible action name. The island says Connected only when the selected audio output is ready. Passive nearby announcements never connect audio.
- **Island motion.** Open over 320 ms and retract over 220 ms using cubic-bezier(.2, 0, 0, 1). Reveal the fully laid-out payload through the rounded capsule geometry; do not fade the header or batteries to conceal overlap. Measure the final size before opening. Hold for four seconds. Content height settles over 180 ms and battery rings over 280 ms. Status text pushes vertically through a fixed, clipped viewport over 200 ms; finish the current push before showing the latest queued event. The action slot appears over 140 ms and disappears over 100 ms. Its old label leaves over 80 ms, followed by a 120 ms entrance for the new label. Hidden and closing controls immediately lose input and accessibility focus. Freeze the outgoing message, battery values and progress angle during dismissal. New announcements reverse the current geometry; ordinary battery updates do not restart dismissal. Render only while moving, including connection progress; reduced motion renders states directly without idle frames. Preserve the pairing card's separate 420 ms entrance.
- **Island connection state.** Keep the island open during an explicit connection request and give the result four seconds. Hide Connect while the backend starts normal playback, with an 18-second limit. An owned case close grants the next opening four seconds for automatic return; repeated lid events must not extend that deadline. A fresh matching close dismisses the battery announcement before stale connection updates arrive. Ignore other pairs and old lid events. Suppress a duplicate announcement after the pairing card succeeds until a real close/open cycle. Listening-mode notices remain independent. The tray's Show batteries action uses existing readings without reconnecting audio.

Use [AGENTS.md](AGENTS.md) for change and review requirements, and
[PRODUCT.md](PRODUCT.md) for platform and feature constraints.

## Do's and Don'ts

- Reuse the native controls, shared theme resources and motion helpers. Preserve keyboard focus, accessible names, high contrast and reduced motion.
- Keep routine status in the control state, tooltip or accessible help. Show actionable errors once and keep active microphone or recovery state visible.
- Keep settings centered, side margins scrollable and the scrollbar at the window edge. State changes must not shift stable content.
- Check long translated labels, different models, missing readings and interrupted animations alongside light, dark and narrow layouts.
- Use the committed images in `docs/images` as appearance references, checking them against the current source before replacing them. Keep README screenshots prominent and the video secondary. Capture sharp images at high resolution and videos with 60 distinct frames per second; leave space between the app and island and remove capture-tool highlights. Preview data stays isolated from real device commands.
