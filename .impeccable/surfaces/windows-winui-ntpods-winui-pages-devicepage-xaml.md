---
version: 1
slug: "windows-winui-ntpods-winui-pages-devicepage-xaml"
primary_target: "windows/winui/NTPods.WinUI/Pages/DevicePage.xaml"
related_targets: ["windows/winui/NTPods.WinUI/MainWindow.xaml","windows/winui/NTPods.WinUI/Pages/SettingsPage.xaml"]
---

# Device settings

Mode: Operate. Native WinUI main window, device page and app settings.

## Layout

Keep the AirPods Dongle card below the shared settings. It stays reachable before
a pair is selected, when the device settings are hidden. The device header
reconnects the saved pair through the selected dongle.
Spatial audio uses a standard navigation row for a selected AirPods Pro 3
pair. Its page groups playback above calibration and ear profile comparison.
The listening room uses a shaded three-dimensional studio to show surround
channels and head movement. Keep the screen fixed in the room while the
camera orbits. Reserve glass for controls around the scene. Live channel
levels belong on the speakers, and exact values and keyboard edits belong
in the native placement controls. When 3D rendering is unavailable, the
native room diagram still supports selection and editing.
Use the same selection highlight for every member of a group. Keep speaker
labels individually selectable when a wide stereo pair overlaps the sides.
Firmware counters appear when experimental controls are enabled.

Use the shared acrylic capsule for Back, More, Connect and phone setup. Pairing
and handoff actions each share a single outlined material. Keep settings and
battery information on the content layer. A 16-DIP scroll-edge fade belongs
below the fixed header and must pass pointer input through to the list.
High contrast removes that fade and uses the user's system colors. Switch
thumbs take on the glass material only while pressed.


The visual authority is LibrePods' Apple `StyledScaffold`, `StyledList`,
`StyledListItem`, `StyledSwitch`, `BatteryView` and Apple settings screens.
Use their bundled Roboto Flex face, centered 22-DIP bold title, 48-DIP round
navigation/actions, filled 28-DIP inset groups and 58-DIP list rows. Keep 16-DIP
side insets and section gaps in a centered 608-DIP maximum outer column.
Subpage bars are 64 DIP high; the main header reserves connection status below
the title. Preserve native Windows input, keyboard, focus and automation behavior.

Cap the hero at 350 DIP: grouped buds on the left, case on the right, and gauges
directly beneath their artwork. Preserve separate per-ear artwork wrappers,
percentages and freshness; near-equal readings must not be merged. Headphones use
one centered image and reading. Name follows the hero immediately and shows its
actual value plus chevron. Group Hearing Health and Press and Hold rows within
filled cards with inset dividers. Standalone navigation rows use the same filled
appearance. The listening rail remains outside any group and retains its glass.

Follow the common order: Name, Hearing Health, Listening Mode, Heart Rate, Press
and Hold AirPods, Call Controls, Audio, ear detection, Microphone Mode, sleep
pause, Optimized Charge Limit, Accessibility, Off Listening Mode and About.
Omit unsupported features. Place companion, Windows volume, playback, microphone,
sensors and checks below these sections under NTPods. Keep one instance of each
control and preserve its pending and cleanup behavior. Use real subpages with a
round Back action and centered title. The header gear opens app settings in one
step. A connected native device with additional actions uses More in that slot.
The dongle shows the pair name in the header, with the editable Name row reserved
for routes that support renaming. App Settings starts with appearance and local
preferences, followed by phone linking.

The scroll area spans the window, including its margins. Keep the scrollbar at
the window edge. Reserve the main status/action row so Connect or Repair cannot
shift the title or body. Keep Connect available for a control-only connection
until its audio output is ready. Settings labels wrap; titles and names trim.

Supported unreported preferences allow an explicit choice. Automatic ear detection
always uses the standard switch, including when showing the user's saved choice.
An unknown charging-case sounds setting uses an unselected native On/Off chooser
in the same 144 DIP slot as the reported switch. Keep sent choices separate from device reports in tooltips and accessible
help. Send paired hold actions only when both choices are valid. Saved choices stay with their pair
and never send automatically when loaded.

Read supported settings automatically on connection. Settings have no Apply or
Read button. Coalesce live gestures with one write in flight and retain the latest
valid choice for the next write. Numeric edits commit only complete, valid values.
Preserve untouched settings from the current pair's confirmed profile; retire
queued edits on failure, a changed pair or connection. Loading a report never writes it.

Reuse Hearing Aid, Headphone Accommodations and Hardware checks through their
subpages for USB dongles. Prefer a connected native session; otherwise use the dongle's own
reported model and session, without exposing unrelated native controls. Changing
the USB target clears drafts and confirmations. Keep the experimental opt-in.

The existing dongle group owns the native board picker. Show it for multiple
boards or a missing selected board, retaining selection by serial across USB
re-enumeration. Keep the group visible for control-only attachment. AirPods link,
control readiness and USB audio availability are separate from route ownership.
Codec/profile edits require idle audio; Game Mode stays live. Reuse the shared
insets, text, native focus and wrapping labels without adding a takeover action
until firmware supplies a guarded request and confirmed ownership readback.

## Headphone Accommodations

Keep the shared path Accessibility → Headphone Accommodations → Transparency.
Follow the shared Apple page and captured controls: master, Audio tuning
choice group, fixed Slight / Moderate / Strong Optimization slider for stock choices, independent
Phone / Media switch rows and Transparency.
Put Audiogram before the three stock tunings, using the saved hearing profile as
its validated source without changing that profile. Hide Optimization for Audiogram.
Unknown curves stay Custom settings; never infer Audiogram or a level from them.
Remember destination choices per pair while off,
including across fresh sessions. Send user changes live. Show the last successful local
choice without a routine warning or reconnect, keeping it separate from device
readback. Fresh reports supersede it; a changed connection discards it.
Omit unverified Customize Audio and Play Sample actions.
Equalizer labels follow the displayed live selection while pending; device
confirmation stays in accessibility help and tooltips. Use the shared 12-DIP
helper size for its description, preset label and band summary.

Transparency uses an independent master group, five independent slider groups
(Amplification, Balance, Tone, Ambient Noise Reduction and Own Voice Amplification),
then a Conversation Boost group. Do not wrap the subpage in another expander. Follow the verified PersonalAudio
conversion; preserve noncanonical custom gains and disable only combined sliders
that cannot represent them. Preserve untouched EQ and per-ear fields. Use live
edits, shared theme resources and native keyboard input.
Keep the five native Sliders and two switches exposed to automation and tab focus.
Transparency and Optimization share the 40-by-24-DIP glass thumb: white at rest, refracting on press, with
a directional rim and a restrained shadow. The five cards use 24-DIP horizontal
and 16-DIP vertical padding with an eight-DIP caption-to-track gap. Keep the full
1.5× pressed thumb inside that inset, with the native high-contrast fallback.
Optimization retains exactly three stops, with labels aligned beneath the track.

## Listening modes

Modes share one draggable glass capsule. Adaptive expands within the rail to
expose its continuous 0–100 strength range. Show Off according to the model and
Allow Off setting; hide it while Adaptive is expanded. Keep requested and
confirmed values separate, coalesce changes during dragging and send the final
value on release. Preserve the geometry and motion documented in DESIGN.md.

## Listen on

Keep Phone connection discoverable in App Settings and as a navigation row under
NTPods. Its setup distinguishes the phone-to-PC Windows Bluetooth bond from
the app link. Use the existing native dialog, paired-phone picker, Bluetooth
settings action and clear code-copy step, with one explicit provisioning action.
Keep temporary linking codes out of logs, persistent UI state, clipboard history
and roaming. Waiting for LibrePods is separate from authenticated connection.
The corresponding LibrePods setting is PC connection.

The selected route supplies the main header and battery hero, including independent
reading ages. Header Connect uses the selected dongle's guarded request when
appropriate. Collapse dongle-specific controls and diagnostics together; show the
board picker when selection is ambiguous or missing. Label Bluetooth audio-source
pairing separately from linking LibrePods. Do not repeat ordinary batteries,
listening modes or reconnect actions inside the dongle group.

Place the compact card below the common device sections, under NTPods, for a
linked phone matching the selected pair. Reuse `Card`, `GlassActionGroup` and `QuietButton`:
12 DIP card padding, equal destination columns with an 8 DIP gap, and Phone and
This PC actions with native icons, wrapping 14-DIP labels and 48-DIP minimum
height. Keep both actions neutral; neither indicates the current audio owner.
Reserve a 20-DIP progress slot beside the title so transfer progress does not
shift the content. Animate it only during an active transfer. Routine pending,
busy and unavailable guidance belongs in tooltips and accessible help. Show an
unconfirmed switch once as an inline error.

Use companion-link readiness independently of PC audio ownership. Require the
selected pair and current session; disable actions during microphone use,
pending playback changes or a transfer. Clear pending requests on pair or session
changes and accept only matching results. After 80 seconds without a result,
request fresh status once before allowing another explicit choice. Never retry
the audio transfer automatically.

## State changes

Use the existing section and artwork transition helpers. Animate actual device
and visibility changes without restarting motion on every snapshot. Keep the
decoded image visible while replacement artwork loads, and keep cached readings
tied to the correct pair. Dim unavailable artwork independently of stale battery
values. Earbuds use three readouts; headphones use one centered readout.
Use the secondary theme brush at full opacity for stale USB battery text so
it remains readable in light, dark and Windows high-contrast appearances.

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

Find nearby is a collapsed native expander with explicit Start and Stop. Show
verified signal strength and trend for the selected pair, with last-heard age.
Dim the retained signal when reports become stale; do not show distance, direction,
or lost-device claims. Stop the view's search on collapse, navigation, window hide,
unload, pair change or daemon loss. Opening the card must never start scanning or
take over audio. Reuse native controls and the existing expander motion in every
theme, including high contrast and reduced motion.

Hardware checks use a collapsed expander and load saved data automatically. Show saved per-ear
results with neutral text and separate device timestamps from the time read.
Cached flags do not establish a current fault. Keep raw device data in secondary
details. Confirm sound-producing in-case requests before sending; show the next
physical action and each ear's new-result progress. Stop waiting only ends host
observation, so it must not claim to stop the firmware check.

Do not show a blanket missing-audiogram or Experimental warning over a validated
stored hearing profile. Missing-profile and failed-write handling must reflect
the actual state; retain backend editing and enrollment guards.

Keep backend behavior, localization, themes and error handling intact. Visual
tests must use an isolated renderer and must not send commands to AirPods.
Compare light, dark, narrow, disconnected and unsupported-feature states. Include
switching pairs, long translated labels, stale readings and interrupted motion.
See docs/WINDOWS-VALIDATION.md for test results.

Reference: LibrePods rewrite commit `34076c85`, `android/imgs/settings-1.png`,
`settings-2.png` and `presentation/components/apple/NoiseControlSettings.kt`.
