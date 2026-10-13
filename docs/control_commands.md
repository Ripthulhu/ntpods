# AACP control commands

The catalog records identifiers extracted from Apple's Bluetooth stack (build
23B5044l). Some are unimplemented or have unverified values. The
[Windows protocol reference](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#setting-values-and-confirmation)
lists NTPods' accepted controls, model gates and confirmation behavior.

Control packets are eleven bytes:

```text
04 00 04 00 09 00 [identifier] [data1] [data2] [data3] [data4]
```

The opcode is `0x0009`, little-endian. Unused value bytes are zero.
Most listed commands use one value byte. Two-byte commands can carry per-bud
settings or paired state fields.

| ID | Reference name | Value bytes |
| --- | --- | --- |
| `0x01` | Mic Mode | `0x00` = Automatic, `0x01` = Right, `0x02` = Left. |
| `0x05` | Button Send Mode | One byte; logged as "Set Button Send Mode: %d", meaning unverified. |
| `0x06` | Owns connection | `0x01` = own, `0x00` = doesn't own. |
| `0x0A` | Ear Detection | `0x01` = enabled, `0x02` = disabled. |
| `0x12` | VoiceTrigger for Siri | `0x01` = enabled, `0x02` = disabled. |
| `0x14` | SingleClickMode | One byte; values unverified. |
| `0x15` | DoubleClickMode | One byte; values unverified. |
| `0x16` | ClickHoldMode | Right byte, then left byte; `0x01` = Noise control, `0x05` = Siri. |
| `0x17` | DoubleClickInterval | `0x00` = Default, `0x01` = Slower, `0x02` = Slowest. |
| `0x18` | ClickHoldInterval | 0x00 = Default, `0x01` = Slower, `0x02` = Slowest. |
| `0x1A` | ListeningModeConfigs | Bitmask, Off mode = `0x01`, ANC=`0x02`, Transparency = `0x04`, Adaptive = `0x08`. |
| `0x1B` | OneBudANCMode | `0x01` = enabled, `0x02` = disabled |
| `0x1C` | CrownRotationDirection | `0x01` = reversed, `0x02` = default. |
| `0x0D` | ListeningMode | 1 = Off, 2 = noise cancellation, 3 = transparency, 4 = adaptive. |
| `0x1E` | AutoAnswerMode | One byte; values unverified. |
| `0x1F` | Chime Volume | First byte 0–100, second byte `0x50` in LibrePods' Android rewrite writer. |
| `0x20` | Connect Automatically | `0x01` = enabled, `0x02` = disabled. |
| `0x23` | VolumeSwipeInterval | `0x01` = Default, `0x02` = Longer, `0x03` = Longest in LibrePods' Android rewrite UI. |
| `0x24` | Call Management Config | `[0x00, 0x03]` = usual single/double-press layout; `[0x00, 0x02]` = flipped. This device setting alone does not integrate call gestures with Windows apps. |
| `0x25` | VolumeSwipeMode | `0x01` = enabled, `0x02` = disabled |
| `0x26` | Adaptive Volume Config | `0x01` = enabled, `0x02` = disabled |
| `0x27` | Software Mute config | One byte; values unverified. |
| `0x28` | Conversation Detect config | `0x01` = enabled, `0x02` = disabled |
| `0x29` | SSL | One byte; values unverified. |
| `0x2C` | Hearing Aid Enrolled and Hearing Aid Enabled | Enrolled byte, then enabled byte; `0x01` = enabled, `0x02` = disabled |
| `0x2E` | AutoANC Strength | 0 to 100. |
| `0x2F` | HPS Gain Swipe (swipe to adjust amplification) | One byte; values unverified. |
| `0x30` | HRM enable/disable state | One byte; values unverified. |
| `0x31` | In Case Tone config | `0x01` = enabled, `0x02` = disabled |
| `0x32` | Siri Multitone config | One byte; values unverified. |
| `0x33` | Hearing Assist config | `0x01` = enabled, `0x02` = disabled |
| `0x34` | Allow Off Option for Listening Mode config | `0x01` = enabled, `0x02` = disabled |
| `0x35` | Sleep Detection config | `0x01` = enabled, `0x02` = disabled |
| `0x36` | Allow Auto Connect | `0x01` = allow, `0x02` = disallow |
| `0x37` | PPE Toggle config | One byte; values unverified. |
| `0x38` | Personal Protective Equipment Cap Level config | One byte; values unverified. |
| `0x39` | Raw Gestures config | Bitmask, single press = `0x01`, double press = `0x02`, triple press = `0x04`, long press = `0x08`. |
| `0x3A` | Temporary Pairing Config | `0x01` = Temporary, `0x02` = Permanent |
| `0x3B` | Dynamic End of Charge config | `0x01` = enabled, `0x02` = disabled in NTPods. |
| `0x3C` | System Siri message config | One byte; values unverified. |
| `0x3D` | Hearing Aid Generic config | One byte; values unverified. |
| `0x3E` | Uplink EQ Bud config | One byte; values unverified. |
| `0x3F` | Uplink EQ Source config | One byte; values unverified. |
| `0x40` | In Case Tone Volume | 0 to 100. |
| `0x41` | Disable Button Input config | One byte; values unverified. |

An identifier's presence in Apple's stack does not establish support on every
AirPods model. Keep unknown values unknown, and require device evidence before
adding them to the Windows write allowlist.
