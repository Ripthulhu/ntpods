# AirPods protocol reference

These documents describe the protocol NTPods implements and the measurements
behind it. Start with the wire format for the feature you are changing, then
check its model and hardware test coverage.

| Reference | Contents |
| --- | --- |
| [Controls, battery and sensors](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md) | AAP connection setup, settings, notifications, nearby BLE battery data, ATT and sensor reports |
| [Playback and microphone](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md) | AAC/AAC-ELD negotiation, media packets, mic uplink, start/stop and recovery |
| [macOS capture procedure](MACOS-PROTOCOL-CAPTURE.md) | Repeatable comparisons with Apple's implementation and a record for unknown traffic |
| [macOS case and sensor captures](MACOS-CASE-CAPTURE-2026-10-12.md) | Battery/charger checks, sensor descriptors, Transparency settings, UARP assets and a primary-bud handoff on Pro 3 |
| [Model support](WINDOWS-MODEL-SUPPORT.md) | Capability profiles and which models have passed hardware checks |
| [Hardware validation](WINDOWS-VALIDATION.md) | Test conditions, results and unresolved failures |
| [Audio timing](WINDOWS-AUDIO-LATENCY.md) | Windows buffering, clocks and latency measurements |
| [Microphone research](WINDOWS-MIC-PROTOCOL-RESEARCH.md) | Earlier upstream findings and the reasons for recovery changes |

## Transport map

| Layer | Traffic | NTPods implementation |
| --- | --- | --- |
| Bluetooth Classic L2CAP, PSM `0x1001` | Apple's accessory protocol, called AAP or AACP in these sources; controls, notifications and HQ microphone packets | [aap.rs](../windows/daemon/src/aap.rs), [AAP driver](../windows/drivers/aap) |
| AVDTP signaling and media | Playback codec selection, stream control and encoded AAC/AAC-ELD | [playback](../windows/daemon/src/playback), [playback guide](../windows/playback/README.md) |
| BLE manufacturer advertisements | Nearby battery and case reports without opening the audio connection | [battery_ble.rs](../windows/daemon/src/battery_ble.rs) |
| ATT | Additional settings, including hearing-assistance data | [device settings](../windows/daemon/src/device_settings.rs), [hearing.rs](../windows/daemon/src/hearing.rs) |
| Standard Bluetooth profiles | Service discovery, media controls and hands-free audio can also appear in a host capture | Mostly handled by Windows; [hands-free visibility](../windows/daemon/src/handsfree.rs) controls which matching endpoints NTPods hides |
| Windows driver/IPC | PCM handoff and application state; these formats are local to NTPods | [IPC](../windows/ipc/src/lib.rs), [driver source](../windows/drivers) |

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
  or capture. It still needs comparison on our hardware when marked unverified.
- **Unknown:** we have bytes or a suspected purpose without enough evidence to
  assign semantics. Keep the uncertainty with the field or command.

Current local checks cover Pro 3 on firmware 9A348 and selected first-generation
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
evidence record, then publish a sanitized description in the validation log.
Use synthetic fixtures for keys, addresses and audio-bearing packets.

Keep unresolved fields visible in the protocol reference. Do not infer a
field's meaning from one correlated event or copy a dissector label into the
supported-feature list without checking the bytes.
