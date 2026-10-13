# AAP definitions and historical observations

The current Windows wire formats are documented in the
[control reference](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md) and
[audio reference](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md). This page retains additional
LibrePods observations from Pro 2 USB-C firmware 7A305 and iOS 26.5.2 / Pro 3
firmware 8B41. Model-specific observations do not establish support on other
firmware.

AAP uses Bluetooth Classic L2CAP PSM `0x1001` (4097). Most packets begin
`04 00 04 00`, followed by a 16-bit little-endian opcode.

## Handshake

```text
00 00 04 00 01 00 02 00 00 00 00 00 00 00 00 00
```

## Setting specific features for AirPods Pro 2

The feature mask below enabled Conversation Awareness during playback and
Adaptive mode in the Pro 2 observations. Individual mask bits are unresolved.

```text
04 00 04 00 4D 00 FF 00 00 00 00 00 00 00
```

## Requesting notifications

The captures contain both masks below. NTPods sends the all-ones mask.

```text
04 00 04 00 0F 00 FF FF FE FF
04 00 04 00 0F 00 FF FF FF FF
```

## Battery

```text
04 00 04 00 04 00 [count] ([component] 01 [level] [status] 01) × count
```

| Component | Value |
| --- | --- |
| Headset (Max) | `01` |
| Right bud | `02` |
| Left bud | `04` |
| Case | `08` |

| Status | Meaning |
| --- | --- |
| `00` | Unknown |
| `01`, `05` | Charging |
| `02` | Discharging |
| `04` | Unavailable |

Level `FF` also means unavailable. The following example reports right 100%
discharging, left 99% charging, and case 17% discharging:

```text
04 00 04 00 04 00 03 02 01 64 02 01 04 01 63 01 01 08 01 11 02 01
```

Parse component IDs rather than assuming record order. Preserve an unavailable
component's last known percentage and its original age; do not display 255%.

## Ear detection

```text
04 00 04 00 06 00 [primary] [secondary]
```

| State | Meaning |
| --- | --- |
| `00` | In ear |
| `01` | Out of ear |
| `02` | In case |
| `03` | Disconnected |
| `04` | Transitional |

Primary and secondary can swap when a bud leaves the ear. They are not fixed
left/right positions. NTPods retains the last stable wear state during a
transitional report.

## Noise control

### Changing noise control

```text
04 00 04 00 09 00 0D [mode] 00 00 00
```

Modes are `01` Off, `02` Noise Cancellation, `03` Transparency and `04` Adaptive.
A matching report confirms the selected mode. Adaptive requires a supported
model, firmware and feature setup; Off availability also depends on the model's
Allow Off setting.

### Adaptive audio noise

```text
04 00 04 00 09 00 2E [level] 00 00 00
```

Level 0 allows more outside sound; 100 filters more. The setting applies in
Adaptive mode. The old Apple UI's three positions do not limit the wire value
to 0, 50 and 100. See the current reference for silent writes and confirmation.

### Configure stem long press

Control `0x1A` sends the complete mode-cycle bitmask: Off `01`, ANC `02`,
Transparency `04`, Adaptive `08`. NTPods requires at least two supported modes.
For example, `0E` selects ANC, Transparency and Adaptive:

```text
04 00 04 00 09 00 1A 0E 00 00 00
```

Another host may change this selection. The observations do not establish
iCloud synchronization or justify replaying an old selection on reconnect.

## Conversational awareness

The event format is:

```text
04 00 04 00 4B 00 02 00 01 [status]
```

Statuses 1/2/3 indicate speech-volume stages; 4/6/7/8/9 restore host volume in
NTPods. These events are separate from the enabled setting:

```text
04 00 04 00 09 00 28 [setting] 00 00 00
```

Setting 1 enables Conversation Awareness; 2 disables it. An initial report does
not guarantee that subsequent writes echo. Keep requested and reported values
separate.

## Metadata

Opcode `0x1D` carries a five-byte information prefix, then eleven NUL-terminated
strings beginning at packet offset 11. See [device information](device-info.md)
for field labels and firmware parsing. The trailing bytes are unparsed; their
presence does not establish encryption. Raw examples contain names and serials
and should remain outside the repository.

## Renaming AirPods

```text
04 00 04 00 1A 00 01 [UTF-8 byte length] 00 [name]
```

The Windows command handler accepts 1–64 UTF-8 bytes and rejects blank names.

## Toggle case charging sounds

The historical Pro 2 capture contains ATT bytes
`12 3A 00 01 00 08 [setting]`, where 0 enabled sounds and 1 disabled them.
This is a fixed-handle observation, not a portable AAP command.
NTPods uses control `0x31` (1 enabled, 2 disabled) on supported models.

<a id="headphone-accomodation"></a>
## Headphone accommodation

Opcode `0x53` has a two-byte length followed by 132 bytes: a four-byte prefix
and four eight-band float32 blocks. The historical template is:

```text
04 00 04 00 53 00 84 00 02 02 [flag1] [flag2] [four blocks of eight float32 values]
```

The floats are little-endian. LibrePods' sender and receiver disagree on which
flag is phone and which is media; captures with equal flags cannot resolve that.
This format differs from NTPods' three-band EQ at `0x63`.

## Customize Transparency mode

The historical ATT handle `0x0018` is model-specific. Use UUID discovery and the
[current 104-byte layout](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#att-settings).
Preserve unknown fields and both EQ curves when changing a supported value.
Amplification/tone ranges of 0–2 and a requirement to follow every write with
`0x53` have not been established by controlled tests.

## Starting and stopping sensor streams

RTBuddy uses opcode `0x17`, a length-delimited protobuf payload and nested
service settings. The iOS / 8B41 observations include two forms:

| Form | Top-level field 2 | Observed service selectors |
| --- | --- | --- |
| A | Absent | `10`, `12`, `13` |
| B | `2` | `50`, `52`, `53` |

The settings record carries a service, report type 2 and five bytes
`01 period_us_LE32`; period zero stops reporting. Sequence varints change the
payload length. Form differences do not establish one universal startup form.
NTPods' current HR writer includes top-level field 2 and addresses services
84/19 after its HR initialization sequence. See
[sensor activation and parsing](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#sensors).

Legacy captures labelled type 16 as raw PPG and type 18 as wear-related activity.
Pro 3 firmware 9A348 descriptors instead identify service 16 as `devmotion6` and
18 as `SPL0`. Use the announced descriptor before assigning meaning to a service
number. Fixed whole-packet offsets are unsafe because wrappers and sequence
varints change.

### Received heart rate data

The 8B41 type-19 record is 18 bytes. BPM is byte 1 and confidence is byte 2.
Its observed counter, state and six-byte time-like field begin at offsets 3, 5
and 6; their general semantics and clock are unverified. Of 62 captured readings,
the four settling records had status tails `10 02 81` or `10 82 81`; the 58
accepted records had `10 00 00`. Keep settling tails out of the accepted list.
The current decoder also validates BPM, confidence, record size and service.

### Head tracking

Use descriptor-matched commands and report-relative channels from the
[current sensor reference](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#sensors).
A fixed service-14 start / service-78 stop pair is not a verified general
sequence. Sensor channels have no established calibrated pose.

## Case and charging transitions

Pro 3 / 8B41 captures show both `01` and `05` during charging. Some sessions
changed from `05` to `01`; one-bud and two-bud sessions also started directly
with `01` and retained it while their percentages rose. Elapsed time, bud count
and case-percentage availability do not predict which value appears.

Control `0x3B=1` appeared 260 ms before one `05` → `01` change; opcode `0x59`
also occurred in that session. Neither correlation establishes the cause.
CaseInfo `0x22/0x23` has no verified battery or lid field. Treat `01` and `05`
as charging without inferring the case's lid or charger state from them.

## Other unresolved messages

Opcode `0x44` has a little-endian length and a variable body. Observed four-byte
bodies begin `02 00 03` and end in 1, 2, 6 or 7. The community dissector calls it
Smart Routing 2.0 information; the captures do not establish a sensor-start
command.

| Control | Captured values | Limit |
| --- | --- | --- |
| `0x0B` | 60, 150 | Reference name Jitter Buffer; [Tahoe's sender](MACOS-AUDIO-CAPTURE-2026-10-12.md#buffer-control-encoding-in-the-saved-daemon) uses millisecond targets and a four-byte value; effects on these captured models remain unverified |
| `0x38` | 82 | Reference name PPE cap level; units and writable range unverified |
| `0x1A/0x32/0x3D` | `0E/01/01` | Observed together during setup; no additional semantics established |

See the [opcode catalog](opcodes.md) and
[Tahoe capture findings](MACOS-CASE-CAPTURE-2026-10-12.md) for other messages.

## License

LibrePods - AirPods liberated from Apple’s ecosystem
Copyright (C) 2025 LibrePods contributors

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Affero General Public License as published
by the Free Software Foundation, either version 3 of the License.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License
along with this program. If not, see <https://www.gnu.org/licenses/>.
