# Windows AirPods controls

The Windows app sends device settings over its existing AirPods control
connection. Native EQ runs on the AirPods. It does not process Windows playback
or replace the selected A2DP driver or codec.

## Confirmed and requested settings

Each connection starts with unknown device settings. A valid AirPods report
supplies the confirmed value. A successful Bluetooth write means the request was
sent; the app waits for a matching report before displaying it as confirmed.
Some firmware reports settings only when the control connection opens again.
Use **Refresh device settings** to request that readback. Refresh is available
while the microphone is idle. The backend checks the driver's live capture count
before reopening the control connection; it rejects requests during recording or
recovery instead of saving them for later.

Most advanced controls remain unavailable until the AirPods report that setting.
EQ also permits an explicit request on firmware beginning with 9 or later,
matching LibrePods' compatibility heuristic. Firmware compatibility alone does
not prove that a setting applied. The app never writes EQ defaults on connection.

Automatic pause is a Windows app preference. Disabling it stops NTPods from
pausing or resuming media; it does not change the AirPods' ear-detection setting.
Automatic resume applies only to the same media session NTPods paused.

Conversation Awareness has a Windows **Volume while speaking** preference. It
caps playback at the selected percentage, with a further reduction during the
second speech stage. The default is 25% (15% in that second stage). It never
raises already-quieter playback. When speech ends or the control session closes,
NTPods restores the original output's volume unless the user changed it during
the conversation. The preference is saved on this PC and does not alter the
AirPods' stored settings.

Adaptive strength is editable only in confirmed Adaptive noise-control mode.
A pending noise-mode change must be confirmed first. AirPods Pro 3 can save
strength without acknowledging the write. The app keeps that requested value
separate until a fresh device report replaces it. Further
strength changes remain available, including returning to the previous value.

AirPods Pro 3 also accepts **Allow "Off" mode** requests without reporting the
setting. The switch is available on known Pro 3 models before its first report.
NTPods saves the last successfully sent choice for that pair and uses it in the
listening controls, tray and stem-cycle options. A real device report replaces
the saved choice. Reconnecting restores the choice without sending another
command. A change made by another host can leave this cache stale until reported.

## Controls in the Windows app

The microphone has three modes. **Automatic** starts when a Windows app records;
its **Stop after** setting delays shutdown after the last app closes the input.
Zero seconds preserves immediate shutdown, and the delay can be set up to one
hour. **Keep active** keeps the AirPods microphone streaming while connected and
enabled, including between recordings. **Manual** provides Start and Stop
buttons; Stop holds even if an app still has the input open. Manual activation
is cleared on restart, disconnect and removal of both AirPods.

The mode and delay are saved on this PC. The microphone switch silences output
immediately in every mode. Audio received without a recording client is
discarded, and each new capture starts with an empty buffer. Stopping still uses
the existing stereo recovery sequence, so choosing an idle delay or Keep active
avoids repeating that sequence between nearby recordings.

| Area | Controls |
| --- | --- |
| Sound | Native three-band EQ, Recommended sound, ANC, Transparency, Adaptive strength, Conversation Awareness with adjustable playback volume, and personalized volume |
| Playback | Automatic pause preference, ear detection, sleep detection and microphone side |
| Stems | Press-and-hold actions and noise-mode cycle, volume swipe, press speed, hold duration, swipe duration and stored call gestures |
| Accessibility and charging | Noise cancellation with one bud, tone volume, case sounds and volume, optimized charging |

Except for the explicit request paths above, unreported settings stay unavailable.
A listed control is not a claim that
every model or firmware supports it. The current hardware checks are below.

## AirPods Pro 3 checks on firmware 9A348

The native EQ roundtrip passed: the WinUI Apply button sent Custom with Bass 53,
Midrange 50 and Treble 50; a fresh AirPods report returned those exact values.
Use recommended restored the original mode 1 and all three values of 50, also
confirmed by the device. The sound itself still needs a listening check.

Ten other controls passed change/readback/restore checks: press speed, hold
duration, one-bud ANC, tone volume, call-gesture preference, volume swipe,
personalized volume, Conversation Awareness, sleep detection and optimized
charging. These are stored-setting checks; physical gestures and acoustic effects
were not tested while the user was away.

With Adaptive mode confirmed, fresh control connections returned strength values
0, 0, 50, 100, 100 and 100 after requests for 0, 10, 25, 75, 90 and 100.
These writes produced no immediate strength report. The coarse readback does
not establish the resolution of the audible effect. The selector retains the
continuous 0–100 range used by Android, with the requested position kept separate
from readback. Native drag and tap checks in 0.1.11 passed, including returning
to the middle after an endpoint. The user confirmed that moving between the
endpoints audibly changes the outside sound, then requested smooth continuous
dragging instead of that build's three-position restriction.

The device did not report microphone side, ear-detection preference, left/right
hold actions, noise-mode cycle, swipe duration, or case-sound settings in these
sessions. Those controls remain unavailable rather than guessing their values.
Allow Off also had no readback, but the 0.1.15 check confirmed that selecting Off
works. Its native switch, silent-request handling, and saved enabled/disabled
choices passed live checks. Adaptive was restored afterward.

## Native EQ protocol

The packet is `04 00 04 00 63 00 05 00 01 mode low mid high`.
Mode 1 selects Recommended; mode 2 selects Custom. Bass, Mid and Treble each
range from 0 to 100, with 50 as neutral. The sources do not establish frequency
or decibel mappings. Incoming EQ reports use the same packet structure.

There is no implemented EQ read request in the pinned sources. NTPods uses the
existing notification subscription and validates incoming reports. EQ is separate
from the experimental eight-band hearing-assistance audiogram.

## Protocol sources

The implementation follows LibrePods Android rewrite commit
`34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c`:

- [Custom EQ type](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/aacp/types/CustomEq.kt)
- [AACP manager](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/aacp/AACPManager.kt)
- [Accessibility controls](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/presentation/screens/apple/AccessibilitySettingsScreen.kt)
- [Device settings](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/presentation/screens/apple/AppleSettingsScreen.kt)

The manager correctly sends EQ opcode `0x63`; the upstream `CustomEqPacket`
class incorrectly labels it `0x09`. The Windows implementation uses `0x63`.
The [Apple Wireshark decoder](https://github.com/pabloaul/apple-wireshark/blob/2e9822247195fad2f79c729010308c33a29546ea/plugins/aacp/aacp_parse_message.lua)
independently documents the EQ packet layout.

## Nearby batteries

The 0.1.19 backend can receive exact battery levels from paired AirPods'
Bluetooth Low Energy advertisements after the audio connection ends. It obtains
the identity and encryption keys over AAP and stores them with current-user
Windows DPAPI. A report must resolve to that identity before it can update the
readings. This does not authenticate the entire advertisement: the encrypted
Apple payload has no message authentication code.

Scanning is passive. By default it pauses while Windows audio or AAP is
connected. The Nearby battery updates setting also permits scanning while
listening; microphone capture and uplink still pause it. A one-minute AX200
playback check with other Bluetooth devices disconnected received 55 verified
battery reports, and the user reported smooth audio. This does not establish
coexistence with other Bluetooth devices or microphone capture.

Reports from both cased buds update batteries without requesting an audio
connection. Windows can still connect independently when the lid opens. The UI
distinguishes nearby battery reports from audio availability. After 90 seconds
without another BLE report, values become dated last-known readings and charging
indicators disappear. The timeout is NTPods' policy, not a measured macOS timeout.
Firmware and radio delivery determine how long the case reports. The AX200 test
received fresh percentages and charging state after the user confirmed the lid
was closed. A roughly one-minute report gap motivated the 90-second timeout;
reporting did not continue indefinitely. Reports while wearing the buds contained
their batteries but no case value.

An unknown component in a later report retains its last known percentage and
original observation time. It becomes a dimmed cached reading with no charging
indicator. Reconnecting the same pair also retains these cached values until new
reports arrive; unrelated bud updates cannot make the old case reading look live.
These values are kept for the running daemon session, not saved across daemon restarts.

The packet layout follows the pinned LibrePods Linux implementation in
`linux/battery.hpp`, `linux/ble/bleutils.cpp`, and `linux/airpods_packets.h`.
The Android rewrite's scan handler remains incomplete at that revision.

## Live head movement

The Head tracking panel follows LibrePods' Apple-style live graph, with separate
horizontal and vertical traces and the most recent 100 samples. Opening it starts
the sensor; Pause freezes the graph. Closing the panel, leaving the device page,
hiding the window, or losing the owning app connection stops the request. Removing
the AirPods suspends it until they are worn again. This preference is not saved.

The backend selects RTBuddy motion service 16 on firmware 8 and later, or service
14 on older firmware, and validates the service and record length before reading
the two signed movement channels. Samples are sent separately from ordinary state
updates, at most 25 times per second. The graph uses raw movement units, matching
the [upstream view](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/presentation/screens/apple/HeadTrackingScreen.kt).
This does not add nod/shake actions or head-tracked spatial audio.

## Limits

The 0.1.14 source comparison and remaining platform gaps are recorded in
[`WINDOWS-UPSTREAM-REVIEW.md`](WINDOWS-UPSTREAM-REVIEW.md). Its new volume preference
and disconnect changes still need live hardware checks; the earlier AirPods
results above do not validate them.

Changing AirPods call or assistant preferences does not connect those gestures
to Discord, Windows call controls or a Windows assistant. Head-gesture call
handling needs host integration. Hearing-assistance and transparency
customization need separate ATT validation; visible Android controls are not
proof of working support because the pinned rewrite's ATT methods are stubs.

Heart rate and multipoint remain experimental. Find My, firmware updates and
spatial audio are not implemented. Hardware checks and remaining release gates
are recorded in `WINDOWS-VALIDATION.md`.
