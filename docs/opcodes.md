# AACP opcodes

AACP (Apple Accessory Communication Protocol) identifies each operation with a 16-bit, little-endian opcode.


| Opcode (Hex) | Destination | Description                                                        |
| ------------ | ----------- | ------------------------------------------------------------------ |
| 0x0001       | Accessory   | Unknown                                                            |
| 0x0004       | Host        | [Battery report](AAP%20Definitions.md#battery)                          |
| 0x0006       | Host        | [Ear detection](AAP%20Definitions.md#ear-detection)                     |
| 0x0009       | Both        | [Control commands](control_commands.md)                      |
| 0x000D       | Accessory   | Audio source req                          |
| 0x000E       | Host        | Audio source resp                         |
| 0x000F       | Accessory   | [Notification register](AAP%20Definitions.md#requesting-notifications)            |
| 0x0010       | Accessory   | Smart routing relay           |
| 0x0011       | Host        | Smart routing response     |
| 0x0014       | Accessory   | Send connected device MAC                                          |
| 0x0017       | Both        | Multiple things - undocumented                                     |
| 0x0019       | Host        | Stem press                                  |
| 0x001B       | Accessory   | Timestamp                                    |
| 0x001D       | Host        | [Device Information](device-info.md)                         |
| 0x001E       | Accessory   | Rename device                                   |
| 0x0022       | Accessory   | Unknown                                                            |
| 0x0029       | Accessory   | Host capabilities (?) |
| 0x002B       | Host        | Paired devices (?)                                                 |
| 0x002D       | Accessory   | List of connected dev. req      |
| 0x002E       | Host        | List of connected devices    |
| 0x0030       | Accessory   | BLE keys req                                  |
| 0x0031       | Host        | BLE keys response                             |
| 0x004B       | Host        | [Conversation awareness](AAP%20Definitions.md#conversational-awareness)        |
| 0x004D       | Accessory   | [Host capabilities](AAP%20Definitions.md#setting-specific-features-for-airpods-pro-2)                    |
| 0x004F       | Both        | Information req/res (doesn't work, even with apple's DID)          |
| 0x0053       | Both        | [EQ data](AAP%20Definitions.md#headphone-accomodation)                                             |