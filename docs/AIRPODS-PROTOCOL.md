# AirPods protocol reference

Wire formats, implementation links and hardware coverage for NTPods.

| Reference | Contents |
| --- | --- |
| [Controls, battery and sensors](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md) | AAP setup, settings, accessory reports, nearby BLE battery and signal finding, DULT case information, ATT and sensors |
| [Playback and microphone](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md) | AAC/AAC-ELD negotiation, media packets, mic uplink, recovery, and the limits of LE Audio/HDR findings |
| [Apple Profile Fast Connect](APPLE-FAST-CONNECT.md) | Discovery, channel assignment, profile descriptors and the Windows driver boundary |
| [macOS capture procedure](MACOS-PROTOCOL-CAPTURE.md) | Repeatable comparisons with Apple's implementation and a record for unknown traffic |
| [macOS audio captures](MACOS-AUDIO-CAPTURE-2026-10-12.md) | Codec negotiation, gaming-audio buffer control and host timing |
| [macOS case and sensor captures](MACOS-CASE-CAPTURE-2026-10-12.md) | Battery/charger checks, sensor descriptors, Transparency settings, UARP assets and a primary-bud handoff on Pro 3 |
| [Model support](WINDOWS-MODEL-SUPPORT.md) | Capability profiles and which models have passed hardware checks |
| [Hardware validation](WINDOWS-VALIDATION.md) | Test conditions, results and unresolved failures |
| [Audio timing](WINDOWS-AUDIO-LATENCY.md) | Windows buffering, clocks and latency measurements |
| [Windows volume bridge](WINDOWS-AVRCP-VOLUME.md) | Native endpoint/AVRCP synchronization, gain mapping and subscription lifetime |
| [Microphone research](WINDOWS-MIC-PROTOCOL-RESEARCH.md) | Pinned upstream sources, clock assumptions and recovery constraints |

## Transport map

| Layer | Traffic | NTPods implementation |
| --- | --- | --- |
| Bluetooth Classic L2CAP, PSM `0x1001` | Apple's accessory protocol, called AAP or AACP in these sources; controls, notifications and HQ microphone packets | [aap.rs](../windows/daemon/src/aap.rs), [AAP driver](../windows/drivers/aap) |
| AVDTP signaling and media | Playback codec selection, stream control and encoded AAC/AAC-ELD | [playback](../windows/daemon/src/playback), [playback guide](../windows/playback/README.md) |
| BLE manufacturer advertisements | Nearby battery, case and identity-verified signal reports without opening audio | [battery_ble.rs](../windows/daemon/src/battery_ble.rs), [nearby_finder.rs](../windows/daemon/src/nearby_finder.rs) |
| BLE GATT, DULT non-owner service | Read-only information from a nearby charging case, separate from the Classic bud connection | [BluetoothCaseInformation.cs](../windows/winui/NTPods.WinUI/Services/BluetoothCaseInformation.cs), [DultCaseProtocol.cs](../windows/winui/NTPods.WinUI/Services/DultCaseProtocol.cs) |
| BLE GATT, FD44 Hawkeye control points | Case sound/configuration research; separate framing and initialization from DULT | No production writer; [sound and bonding limits](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#case-sound-and-bonding-limits) |
| ATT | Additional settings, including hearing-assistance data | [device settings](../windows/daemon/src/device_settings.rs), [hearing.rs](../windows/daemon/src/hearing.rs) |
| Standard Bluetooth profiles | Service discovery, media controls and hands-free audio can also appear in a host capture | Mostly handled by Windows; NTPods leaves hands-free endpoints available for [manual configuration](WINDOWS-FEATURES.md#microphone) |
| Windows driver/IPC | PCM handoff and application state; these formats are local to NTPods | [IPC](../windows/ipc/src/lib.rs), [driver source](../windows/drivers) |
| USB HID to Pico 2 W | Bounded AACP/ATT settings transactions on the dongle's selected Classic link | [dongle_rpc.rs](../windows/daemon/src/dongle_rpc.rs), [dongle_health.rs](../windows/daemon/src/dongle_health.rs) |

The USB hearing/diagnostic path reuses the native record parsers and complete
profile readback checks. It keeps separate state pinned to the USB session and
Bluetooth generation. USB acceptance is not device confirmation. No test-signed
Windows driver is required for this path, but Pico hardware validation remains
pending; native Bluetooth results do not establish dongle support.

Hearing and diagnostic workflows require the dongle's settings-lease capability
(`0x20`). One client holds it from baseline reads through writes, verification and
mode cleanup, so concurrent phone edits cannot overwrite the PC's changes. An
expired lease cannot be reacquired halfway through an edit. Older firmware needs
an update; unlocked raw transactions are rejected. Queued typed setting writes
complete only after local transmission and still need device readback.

An HCI trace shows the host/controller boundary. It can expose L2CAP and media
payloads after Bluetooth link decryption, but it does not reveal all traffic
between the earbuds or the earbuds and case. Case state reported through a bud
is evidence of that report, not a capture of the case's own radio exchange.

## How to read the evidence

- **Implemented:** the current parser, sender or state machine handles it.
  A regression test establishes that code's behavior on its supplied input.
- **Hardware checked:** the validation record names the model, firmware and
  observed outcome. A successful write alone does not confirm a setting changed.
- **Reference-derived:** the mapping comes from a named upstream implementation
  or capture. It still needs hardware comparison when marked unverified.
- **Unknown:** bytes or a suspected purpose without enough evidence to assign
  semantics. Keep the uncertainty with the field or command.

Recorded hardware checks cover Pro 3 on firmware 9A348 and selected first-generation
Pro features. A shared parser or catalog entry does not establish the same
behavior on every AirPods generation. Consult the model table for each feature.

The older [AAP definitions](AAP%20Definitions.md), [opcode list](opcodes.md),
[control list](control_commands.md) and [device information](device-info.md)
retain useful upstream material. Prefer the current Windows references above
when those older tables differ from the implementation.

## Updating a decoded field

Record its direction, transport, opcode, offset, width, byte order, valid values
and length checks. Link the parser or sender and its regression case. For a
hardware claim, add the model/firmware and capture frame numbers to the private
evidence record, then publish a sanitized result in the validation matrix.
Use synthetic fixtures for keys, addresses and audio-bearing packets.

Keep unresolved fields visible in the protocol reference. Do not infer a
field's meaning from one correlated event or copy a dissector label into the
supported-feature list without checking the bytes.
