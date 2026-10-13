# AACP opcodes

AACP (Apple Accessory Communication Protocol) identifies each operation with a 16-bit, little-endian opcode.

This research catalog includes commands NTPods does not implement. Names follow
the [pinned AACP reference](https://github.com/pabloaul/apple-wireshark/blob/2e9822247195fad2f79c729010308c33a29546ea/plugins/aacp/aacp_enums.lua)
where the implementation has no established meaning. Use the
[Windows protocol reference](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md) for current
writers and parsers, and the [macOS capture findings](MACOS-CASE-CAPTURE-2026-10-12.md)
for observed but unimplemented messages.

| Opcode (Hex) | Destination | Description                                                        |
| ------------ | ----------- | ------------------------------------------------------------------ |
| 0x0001       | Accessory   | Capability request                                                 |
| 0x0002       | Host        | Accessory capabilities                                             |
| 0x0004       | Host        | [Battery report](AAP%20Definitions.md#battery)                          |
| 0x0006       | Host        | [Ear detection](AAP%20Definitions.md#ear-detection)                     |
| 0x0008       | Host        | Primary bud role; remaining flags incompletely decoded              |
| 0x0009       | Both        | [Control commands](control_commands.md)                      |
| 0x000D       | Accessory   | Audio source req                          |
| 0x000E       | Host        | Audio source resp                         |
| 0x000F       | Accessory   | [Notification register](AAP%20Definitions.md#requesting-notifications)            |
| 0x0010       | Accessory   | Smart routing relay           |
| 0x0011       | Host        | Smart routing response     |
| 0x0014       | Accessory   | Send connected device MAC                                          |
| 0x0017       | Both        | RTBuddy: sensor descriptions, settings, reports and acknowledgments |
| 0x0019       | Host        | Stem press                                  |
| 0x001A       | Accessory   | Rename device                                                      |
| 0x001B       | Accessory   | Timestamp                                    |
| 0x001D       | Host        | [Device Information](device-info.md)                         |
| 0x001E       | Accessory   | External Accessory session packet (reference label)                 |
| 0x0022       | Accessory   | CaseInfo request                                                   |
| 0x0023       | Host        | CaseInfo response; field meanings incomplete                        |
| 0x0029       | Accessory   | Country code (reference label)                                     |
| 0x002B       | Host        | Stream State Info (reference label); bounded diagnostics in NTPods  |
| 0x002D       | Accessory   | List of connected dev. req      |
| 0x002E       | Host        | List of connected devices    |
| 0x0030       | Accessory   | BLE keys req                                  |
| 0x0031       | Host        | BLE keys response                             |
| 0x004B       | Host        | [Conversation awareness](AAP%20Definitions.md#conversational-awareness)        |
| 0x004C       | Host        | Adaptive Volume message; payload meanings unknown                   |
| 0x004D       | Accessory   | [Host capabilities](AAP%20Definitions.md#setting-specific-features-for-airpods-pro-2)                    |
| 0x004E       | Host        | Proximity-card status (reference label)                             |
| 0x004F       | Both        | UARP information, asset and firmware protocol                        |
| 0x0053       | Both        | [Headphone Accommodation](AAP%20Definitions.md#headphone-accomodation); separate from 0x0063 EQ |
| 0x0054       | Accessory   | Radio-band edges; units unknown                                     |
| 0x0055       | Host        | Unknown                                                            |
| 0x0058       | Both        | [High-quality microphone](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md)          |
| 0x0063       | Both        | Three-band EQ                                                      |
