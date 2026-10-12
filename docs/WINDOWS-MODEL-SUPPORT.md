# Device model support on Windows

The shared [model catalog](../windows/device-models.json) defines which controls
NTPods can offer. Rust and WinUI read the same file. Listed models have not all
been tested on hardware.
The recorded Windows hardware checks used AirPods Pro 3, firmware 9A348, and an
Intel AX200. First-generation Pro A2084 has also passed connection, battery,
listening-mode, automatic-pause and native AAC/AAC-ELD playback checks on that
adapter. Additional checks are
recorded in WINDOWS-VALIDATION.md; other models remain unverified on hardware.

The native playback driver currently binds only Pro 1 and Pro 3. Codec selection
uses each pair's advertised AVDTP endpoints and bitrate limits. Both pairs passed
AAC at 256 kbps and AAC-ELD at 128, 192 and 256 kbps, including a continuous
70-second AAC-ELD run. Other models keep their existing playback driver until
their native playback support can be checked. Playback support does not enable
the high-quality microphone on older models.

| Models | Listening modes | Live movement | High-quality microphone / custom EQ |
| --- | --- | --- | --- |
| AirPods 1 and 2 | None | Disabled | Disabled |
| AirPods 3 | None | Available to test | Disabled |
| AirPods 4 | None | Available to test | Firmware-gated |
| AirPods 4 with ANC | Off, Transparency, ANC, Adaptive | Available to test | Firmware-gated |
| AirPods 5, both cases | Off, Transparency, ANC, Adaptive | Available to test | Firmware-gated |
| AirPods Pro 1 | Off, Transparency, ANC | No data in the 6F21 hardware check | Disabled |
| AirPods Pro 2, both cases | Transparency, ANC, Adaptive; Off follows the device's policy | Available to test | Firmware-gated |
| AirPods Pro 3 | Transparency, ANC, Adaptive; Off follows the device's policy | Tested on the setup above | Mic audio and EQ setting roundtrips tested |
| AirPods Max 1, both ports | Off, Transparency, ANC | Available to test | Disabled |
| AirPods Max 2 | Off, Transparency, ANC, Adaptive | Available to test | Firmware-gated |
| Powerbeats Pro 2 | Off, Transparency, ANC, when reported | Disabled | Disabled |

High-quality microphone requests require an eligible model and firmware major
8 or newer. Custom EQ requires an eligible model and firmware major 9 or newer.
These follow LibrePods' recorder and EQ gates, with the model check added to
avoid enabling newer streams on older hardware. Max 2 has Apple's studio-quality
recording feature, but its NTPods microphone stream has not been tested.
The catalog limits heart-rate controls to Pro 3 and hearing-aid controls to
Pro 2 and Pro 3. Loud Sound Reduction has its own capability: Pro 2, Pro 3 and
Max 2 may expose it without giving Max 2 hearing-aid controls. Its Windows path
still needs a Max 2 check.

AirPods 5 has separate entries for the USB-C case (A3531/A3532/A3533) and wireless
charging case (A3439/A3440/A3441). Only the latter exposes volume swipe and case
speaker controls. Neither inherits Pro heart-rate, hearing-aid or Loud Sound
Reduction support. Sleep detection requires a reported setting and recognized
firmware major 8 or newer; the initial AirPods 5 release is 9A350.
Off is available by default, but an explicit valid Allow Off report takes
precedence. Missing or invalid reports do not disable that default.

AirPods 1 and 2 do not expose Optimized Battery Charging. Their double-tap
configuration remains unavailable: the command identifiers are documented, but
the reviewed sources do not establish the action-value encoding. Max models can
offer Digital Crown direction and call controls when the device reports them;
Max 2 can also offer sleep detection under the same report and firmware gates.
The earbud microphone-side selector stays unavailable on Max: its command
semantics have not been verified for an over-ear headset.

Powerbeats Pro 2 model A3157 has a conservative entry for battery, reported
ear detection and listening modes. Its heart-rate hardware does not establish
compatibility with NTPods' sensor path. Movement, high-quality microphone, custom
EQ and hearing features remain disabled. Other Beats models retain the unknown
device policy until their model identities and supported commands are verified.

Ordinary setting writes also require a supported control and a value reported by
the connected device. The existing Pro 3 Allow Off exception keeps a successfully
sent request separate from a device confirmation. Unknown models retain
conservative report-based controls; newer microphone, EQ, heart-rate and movement
streams stay disabled.

Live movement uses sensor service 14 before firmware 8 and service 16 on newer
firmware, following LibrePods. The decoder accepts the 58-byte movement record
and rejects the shorter activity records. Movement support on Pro 1 and Max 1
comes from their head-tracking hardware; LibrePods' separate `HEAD_GESTURES` flag
is not a complete list of models with motion sensors. Windows movement data on
these older models still needs validation. Pro 1 A2084 on 6F21 timed out without
accepted movement samples, so its movement support remains unresolved. One
protocol difference to investigate is the rewrite's AACP 1.3 initialization;
NTPods starts its ordinary control session with 1.2. Max 2 also needs sensor
descriptor discovery: another Windows implementation found its motion stream on
advertised service 6. Motion hardware alone does not establish NTPods graph
support. NTPods does not provide head-tracked spatial playback.

Firmware gates accept public build strings and legacy dotted versions. Metadata
also decodes release-only packed values such as `6619472` to `6F21`; that layout
is inferred from matching release reports, with unknown prerelease forms left
unknown. Internal build numbers must not enable newer microphone or sensor paths.

Pro 1 press-speed changes applied on the test pair, but did not echo or replay
after another notification request. Refresh device settings reads the value
through a new control session. NTPods keeps those requests unconfirmed until a
real report arrives. NTPods no longer probes any model with an unchanged listening
mode: a missing echo does not prove a transport failure. Quiet connections remain
usable, and a recovered driver status clears a transport warning.

## Battery reports and device changes

Connected reports carry either left/right/case batteries or a single Max headset
battery. Nearby BLE reports must match the selected pair's protected identity
key. Keys are saved per pair; the old single-pair file is retained during
migration. Switching pairs must clear the previous pair's live values and pending
commands before accepting reports for the newly selected device.

The Max Lightning and USB-C BLE decoder follows LibrePods' separate headset
branch. It has synthetic parser coverage, but no physical Max capture has been
checked here. Max advertisements update batteries only: automatic reconnect waits
for verified wear and Smart Case flags. The Max 2 BLE model identifier and payload
remain unverified. A sleeping accessory may stop advertising; cached values keep
their original age and must not appear to be fresh readings.

AirPods 5 BLE product IDs `0x3020` and `0x3620` use the existing earbud battery
layout. The decoder covers both with independently encrypted synthetic fixtures,
including charging, absent components, primary-side changes and address checks.
The sources establish both as AirPods 5 family IDs, but do not establish which
case variant each ID denotes; BLE IDs do not select case-specific capabilities.
The proposed Max 2 ID `0x2D20` and Powerbeats Pro 2 SDP product ID `0x201D` do not
provide captured BLE battery-layout evidence, so neither is added to the BLE
decoder. Powerbeats battery reports over an active AAP connection remain usable.

## First-generation Pro check

1. Close the Pro 3 case. Pair and connect the Pro 1 earbuds in Windows. NTPods
   should follow the connected pair. Confirm model A2083 or A2084 and their own
   firmware and name.
2. Confirm that left, right and case readings belong to this pair. Open the case
   with both earbuds inside, then remove one; the last known case percentage
   should remain visible with the correct freshness.
3. Try Off, Transparency and Noise Cancellation. Check the audible change and
   confirmed mode after reconnecting. Adaptive, Conversation Awareness, heart
   rate, custom EQ and the high-quality microphone must stay unavailable.
4. Play music and remove one earbud. Check automatic pause, then reinsertion.
   Music paused manually must not resume merely because the earbuds return.
5. If the earbuds report stem/accessibility settings, change one, confirm the
   result, and restore it. Unreported settings must remain unavailable.
6. Open live movement and turn/nod gently. Record whether data arrives and stops
   when the section closes. A working graph is a separate pass from audio.
7. Close the case and check nearby battery updates if this pair provides its
   keys. Let reports expire and confirm stale values do not claim a live link.
8. Switch back to Pro 3, then Pro 1. Check identities, available controls,
   batteries and saved keys. Restart NTPods and repeat the pair selection.

Record these results in [WINDOWS-VALIDATION.md](WINDOWS-VALIDATION.md), including
firmware versions and any failures. Leave microphone and movement support marked
unverified on other models until their own checks pass.

## Sources

Checked on 2026-10-12. LibrePods `android/rewrite` resolved to
`34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c`; `main` resolved to
`efccc4499b2c1fd2ac243e975ae1f42d8e828ffd`.

- [LibrePods model numbers and base capabilities](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/devices/AirPods.kt).
- [Recorder, EQ and setting gates](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/presentation/screens/apple/AppleSettingsScreen.kt).
- [Legacy and current movement services](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/devices/AppleDevice.kt#L263).
- [BLE model identifiers](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/linux/ble/blemanager.cpp#L8) and [separate Max battery decoding](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/linux/battery.hpp#L157).
- Apple: [model identification](https://support.apple.com/en-us/109525), [Spatial Audio and head tracking](https://support.apple.com/guide/airpods/dev00eb7e0a3/web), and [Max 2 recording capabilities](https://www.apple.com/newsroom/2026/03/apple-introduces-airpods-max-2-powered-by-h2/).
- Apple: [AirPods 5 specifications and case differences](https://www.apple.com/airpods-5/specs/), [firmware features including sleep detection](https://support.apple.com/en-us/108764), and [Max settings including Digital Crown direction and sleep detection](https://support.apple.com/guide/airpods/change-settings-for-airpodsmax-dev3e3e8821d/web).
- [AirPods 5 BLE family identifiers](https://github.com/SpriteOvO/AirPodsDesktop/blob/71fbb5e961991dd614e2c58bbadfb918b9441e92/Source/Core/AppleCP.cpp) and [sanitized battery captures](https://github.com/SpriteOvO/AirPodsDesktop/blob/71fbb5e961991dd614e2c58bbadfb918b9441e92/Tests/AirPodsDomainTests.cpp).
- [Powerbeats Pro 2 features](https://www.beatsbydre.com/earbuds/powerbeats-pro-2) and [A3157 AAP research](https://github.com/mahdi-salmanzade/BeatsBar/blob/c74476694f84cd75e9cbd9e44c7bd5c135541ce0/research/PROTOCOL.md#L42).
- [LibrePods labels high-quality two-way audio experimental](https://github.com/librepods-org/librepods/blob/efccc4499b2c1fd2ac243e975ae1f42d8e828ffd/README.md#high-quality-two-way-audio).
- [Packed Pro 1 firmware report](https://github.com/librepods-org/librepods/issues/769) and [packed 9A348 report](https://github.com/librepods-org/librepods/issues/782).
- [Max 2 sensor discovery and untested Pro 1 support](https://github.com/FIocker/aap-head-tracker/blob/767bbd37b93979084bdcc3e4dc9b9f13c72f82ce/README.md#L106).
