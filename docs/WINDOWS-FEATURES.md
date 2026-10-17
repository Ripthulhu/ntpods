# Windows AirPods controls

Controls depend on the connected model, firmware and settings reported by the
AirPods. See the [model list](WINDOWS-MODEL-SUPPORT.md) for capability gates and
[validation results](WINDOWS-VALIDATION.md) for hardware coverage.

| Area | Controls |
| --- | --- |
| Sound | Noise cancellation, Transparency, Adaptive strength, Conversation Awareness, personalized volume and native three-band EQ |
| Playback and microphone | AAC/AAC-ELD codec and bitrate, ear detection, sleep detection and microphone side |
| Microphone | Automatic, Keep active, Manual Start/Stop and a keyboard shortcut |
| Stems | Press-and-hold actions, noise-mode cycle, volume swipe, press speed, hold/swipe duration and stored call gestures |
| Accessibility and charging | One-bud ANC, tone and case volume, case sounds, optimized charging and experimental Loud Sound Reduction |
| Sensors and hearing | Heart rate, live head movement, experimental Hearing Aid, Media Assist and Custom Transparency |

Automatic pause is a local NTPods preference. It resumes only the same media
session that NTPods paused. Editing an AirPods call or assistant preference does
not connect that gesture to Discord, Windows call controls or a Windows assistant.

## Confirmed and requested settings

Each connection starts with unknown device settings. A valid AirPods report
supplies the confirmed value. A successful Bluetooth write means the request was
sent; the app waits for a matching report before showing it as confirmed.
Some firmware reports settings only when the control connection opens again.
Use **Refresh device settings** for that readback while the microphone is idle.
The backend rejects refresh during recording or recovery rather than queuing it.

Most advanced controls remain unavailable until the AirPods report the setting.
Native EQ also permits an explicit request on eligible models with firmware major
9 or newer, following LibrePods' compatibility gate. The app never writes EQ
defaults on connection.

Adaptive strength uses a continuous 0–100 slider. Adaptive mode must be confirmed
before NTPods sends a strength request. Pro 3 firmware 9A348 can save strength
without acknowledging the write, so the app retains the requested position
separately until a fresh report replaces it. Reconnect reports were coarse in the
recorded test; they do not establish the resolution of the audible effect.

Known Pro 3 models also accept **Allow "Off" mode** before reporting that setting.
NTPods saves the last successfully sent choice for that pair and uses it in the
listening controls, tray and stem-cycle options. A real report replaces the saved
choice. Reconnecting restores the choice without sending another command. A
change made on another host can leave this cache stale until reported.

## Conversation volume

**Volume while speaking** caps Windows playback at the chosen percentage, with
a further reduction during the second speech stage. The default is 25% (15% in
that second stage). It never raises already-quieter playback. When speech ends
or the control session closes, NTPods restores the original output's volume unless
the user changed it during the conversation. This preference is saved on the PC;
the AirPods' Conversation Awareness switch is a separate device setting.

## Microphone

Enable **Use NTPods microphone**, then select the input named after your AirPods
with **(NTPods)** at the end in the recording app. Before the endpoint is renamed,
it may appear as **Microphone (NTPods)**.

| Mode | Behavior |
| --- | --- |
| Automatic | Starts when an app records. **Stop after** delays shutdown after the last app closes the input, from zero seconds to one hour. |
| Keep active | Keeps the microphone streaming while connected and enabled, including between recordings. |
| Manual | Start and Stop control the stream directly. Stop holds even if an app still has the input open. |

The mode and delay are saved on this PC. Manual start intent clears on restart,
disconnect and removal of both AirPods. The permission switch silences the input
in every mode. Audio received without a recording client is discarded, and each
new capture starts with an empty buffer.

The optional global shortcut starts/stops the microphone and selects Manual mode.
Microphone changes appear in the Dynamic Island. NTPods must be running for the
shortcut to work. The shortcut requires Ctrl or Alt with a key; Windows-key
combinations and F12 are unavailable.

Stopping the stream runs playback recovery. An idle delay or Keep active avoids
repeating that interruption between nearby recordings. With the native playback
driver and a working NTPods microphone route, the app hides the pair's hands-free
endpoints to reduce duplicate device choices. It restores them when that route
is no longer available or NTPods exits.

## Nearby batteries

NTPods can receive exact battery levels from paired AirPods' Bluetooth Low Energy
advertisements after the audio connection ends. It obtains identity and encryption
keys over AAP and protects them with current-user Windows DPAPI. A report must
resolve to the selected pair's identity before updating the readings. This does
not authenticate the entire advertisement: Apple's encrypted payload has no
message authentication code.

Scanning is passive and pauses during microphone capture or uplink. By default,
it also pauses while Windows audio or AAP is connected. **Nearby battery updates**
permits scanning during playback; coexistence has only a short AX200 check with
other Bluetooth devices disconnected.

Reports from cased buds update batteries without requesting an audio connection.
Windows can still connect independently when the lid opens. After 90 seconds
without another BLE report, readings become dated and charging indicators
disappear. The timeout is NTPods' policy. Firmware and radio delivery determine
how long the case reports; charger insertion with a sleeping, closed case did not
produce a fresh report in the recorded check.

An unknown component in a later report retains its last known percentage and
original observation time, with dimmed values and no charging indicator.
Reconnecting the same pair retains that cache until new reports arrive. Bud-only
updates cannot make an old case reading look fresh. The cache lasts for the
running daemon session, not across daemon restarts.

The BLE layout follows LibrePods' pinned Linux implementation in `linux/battery.hpp`,
`linux/ble/bleutils.cpp` and `linux/airpods_packets.h`; see the
[model sources](WINDOWS-MODEL-SUPPORT.md#sources).

## Sensors and hearing

The Head tracking panel shows horizontal and vertical raw movement and the most
recent 100 samples. Opening it starts the sensor; Pause freezes the graph.
Closing the panel, leaving the device page, hiding the window or losing its
owning app connection stops the request. Removing the AirPods suspends it until
they are worn again. This preference is not saved.

The backend uses an advertised RTBuddy motion service when available. Otherwise
it falls back to service 16 on firmware 8 and later, or 14 on older firmware.
It validates the service, packet envelope and record length before reading the
two signed channels. UI samples are sent separately from ordinary state updates,
at most 25 times per second. The graph follows the
[LibrePods view](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/presentation/screens/apple/HeadTrackingScreen.kt).
The values are not calibrated angles; nod/shake actions and spatial audio are
not implemented.

Heart rate, Hearing Aid, Media Assist and multipoint remain experimental. Custom
Transparency offers per-ear amplification, tone, ambient-noise reduction and
Conversation Boost after reading the current settings. On Pro 3, the
[Apple Bluetooth identity](BLUETOOTH-HOST-IDENTITY.md) allowed reads, writes
and an audible Custom Transparency adjustment, with restoration confirmed by
readback. Hearing Aid and Media Assist can switch an existing iPhone-fitted
profile independently on Pro 3 / 9A348. NTPods reads and preserves its separate
eight-band audiogram; the captured fitting formats remain read-only. The mode
switches require confirmed device reports, and other models remain untested.
Neither feature is the native three-band EQ. See the
[control protocol](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#att-settings).

Find My and firmware updates are not implemented. SPL0 sensor messages have no
verified measurement scale and no UI. See the
[protocol reference](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md) for decoded messages and
[upstream comparison](WINDOWS-UPSTREAM-REVIEW.md) for remaining platform gaps.

## Native EQ protocol

The packet is `04 00 04 00 63 00 05 00 01 mode low mid high`.
Mode 1 selects Recommended; mode 2 selects Custom. Bass, Mid and Treble each
range from 0 to 100, with 50 as neutral. The sources do not establish frequency
or decibel mappings. Incoming reports use the same structure. EQ runs on the
AirPods and does not change the Windows codec or playback driver.

There is no implemented EQ read request in the pinned sources. NTPods uses the
existing notification subscription and validates incoming reports.

The implementation follows LibrePods Android rewrite commit
`34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c`:

- [Custom EQ type](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/aacp/types/CustomEq.kt)
- [AACP manager](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/aacp/AACPManager.kt)
- [Accessibility controls](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/presentation/screens/apple/AccessibilitySettingsScreen.kt)
- [Device settings](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/presentation/screens/apple/AppleSettingsScreen.kt)

The manager sends EQ opcode `0x63`; the upstream `CustomEqPacket` class labels it
`0x09`. NTPods uses `0x63`, also documented by the
[Apple Wireshark decoder](https://github.com/pabloaul/apple-wireshark/blob/2e9822247195fad2f79c729010308c33a29546ea/plugins/aacp/aacp_parse_message.lua).
