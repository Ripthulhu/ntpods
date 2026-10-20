# AirPods control, battery and sensor protocol

This reference describes NTPods' reverse-engineered Windows implementation.
Packet offsets are zero-based; hex values have a `0x` prefix. `H → B` means host
to buds and `B → H` means buds to host.

“Implemented” below means the code constructs or accepts that format. Hardware
evidence is listed separately, by model and firmware. A successful Bluetooth write
does not prove that a setting changed. See the [protocol index](AIRPODS-PROTOCOL.md),
[audio protocol](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md), and
[validation record](WINDOWS-VALIDATION.md) for the other parts of the system.

## Transport and session setup

NTPods opens an authenticated, encrypted Bluetooth Classic L2CAP channel to the
selected paired device at PSM `0x1001`. The AAP driver uses Basic mode, requests an
inbound MTU of 1691 bytes (minimum 672), and queues ten inbound MTUs. ATT uses a
separate channel at PSM `0x001F`, opened on its first request. Neither is BLE GATT
over a low-energy connection. Windows owns the Bluetooth radio and link security.
See [L2cap.c](../windows/drivers/aap/L2cap.c) and the
[driver contract](../windows/drivers/aap/README.md).

[`run_receiver`](../windows/daemon/src/main.rs) sends this sequence on each new
control connection:

| Direction | Bytes | Use in NTPods |
| --- | --- | --- |
| H → B | `00 00 04 00 01 00 02 00 00 00 00 00 00 00 00 00` | AACP 1.2 handshake; wait 300 ms |
| H → B | `04 00 04 00 4D 00 FF 00 00 00 00 00 00 00` | Feature mask; wait 300 ms |
| H → B | `04 00 04 00 0F 00 FF FF FF FF` | Subscribe to notifications |
| H → B | `04 00 04 00 01 00 08` | Request the capability list |
| H → B | `04 00 04 00 24 00 02 00 01 00 10` | Identify the source as Mac |
| H → B | `04 00 04 00 30 00 05 00` | Request keys for paired-device BLE battery reports |

The `0x4D` mask enables features used by Adaptive and Conversation Awareness on
tested devices. The meaning of every mask bit is unknown. The delays are current
host behavior, not measured protocol requirements. NTPods marks its control
connection up after sending this sequence; individual reports establish settings.

Opcode `0x24` carries a little-endian device-info subtype (`2`), payload length (`1`),
and source type (`0x10` for Mac). Tahoe sends it after feature setup and before audio
Start. Its effect on AAC-ELD timestamp handling is unverified.

Most control packets then have `04 00 04 00`, a 16-bit little-endian opcode, and an
opcode-specific payload. There is no universal payload length field. Preserve
L2CAP packet boundaries when inspecting captures. Sensor frames have their own
length and reassembly; concatenating arbitrary control packets before parsing
would change what the current parsers see.

## Implemented message families

The builders and parsers below are in [aap.rs](../windows/daemon/src/aap.rs) unless
another file is named.

| Opcode | Direction | Payload or handling |
| --- | --- | --- |
| `0x0001` | H → B | Capability request with captured selector `08` |
| `0x0002` | B → H | Capability count, then fixed-width ID/value entries |
| `0x0004` | B → H | Battery count and five-byte component records; see below |
| `0x0006` | B → H | Primary and secondary ear states at offsets 6 and 7 |
| `0x0008` | B → H | Exactly four body bytes: primary role and three unassigned flag bytes |
| `0x0009` | Both | One-byte setting ID, then a four-byte little-endian value; exactly 11 bytes for `parse_device_control` |
| `0x000F` | H → B | Four-byte notification mask; the normal writer sends all bits set |
| `0x0017` | Both | RTBuddy/SensorDataWX framing; see sensor section |
| `0x001A` | H → B | Rename: `01 length 00`, then UTF-8 bytes (`build_rename`); command handler allows 1–64 bytes and rejects blank names |
| `0x001D` | B → H | Device metadata; strings begin at offset 11 (`parse_metadata`) |
| `0x0023` | B → H | Captured 25-byte CaseInfo body; bounded version-3 metadata decoder, unknown forms retained as opaque bytes |
| `0x002E` | B → H | Connected-host list; count at offset 8, entries begin at 9 |
| `0x0030` | H → B | BLE key request payload `05 00` |
| `0x0031` | B → H | BLE key response; never forwarded to UI or general packet logs |
| `0x004B` | B → H | Conversation Awareness event; status at offset 9 |
| `0x004C` | B → H | uint16 LE length followed by exactly that many bytes, at most 256; Adaptive Volume targets or DRC report |
| `0x004D` | H → B | Feature mask from session setup |
| `0x0055` | B → H | Exactly four body bytes; meaning unknown |
| `0x0058` | Both | High-quality microphone control/audio; [audio reference](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md) |
| `0x0063` | Both | Three-band EQ: `05 00 01 mode low mid high`; exactly 13 bytes |

[`capabilities.rs`](../windows/daemon/src/capabilities.rs) keeps reported capability
IDs and raw values in the IPC `aacp_capabilities` map for the current control
connection. IDs `03`–`08` and `30` carry uint32 LE values; other recognized IDs
carry one byte. Widths follow the pinned LibrePods
[table](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/aacp/types/Capability.kt).
Entries have no length field, so unknown widths, duplicate IDs,
truncated values and trailing bytes are rejected without replacing the previous
report. Reconnecting clears the map. Values use different schemes across IDs;
this map does not change feature availability or activate hearing modes.

The read-only [accessory parser](../windows/daemon/src/aap/accessory.rs) also
validates `08`/`23`/`4C`/`55` reports under the service-4 header.
The receive path logs changed reports as lengths or decoded metadata, without
case names, opaque identity or payload bytes. These reports do not grant
feature support, select an audio route or send commands.

CaseInfo's verified version-3, 25-byte body contains a uint16 vendor ID, uint32
product ID, uint16 vendor source, color byte, split uint64 version, length-prefixed
NUL-terminated name and uint32 reserved value. Integers are little-endian.
The decoder checks the complete envelope, name length, terminator and UTF-8.
Other versions or malformed layouts stay opaque. The raw product ID is not the
case's standard PnP product ID, and its raw version is not the DULT firmware value.
No field establishes the case's BLE address or a unique association with the buds.

The iOS 27.0.1 (24A446) wire parser establishes these offsets, relative to the
body after the six-byte AAP header. `N` excludes the name's terminating NUL;
`E = 14 + N` starts the extension:

| Body offset | Width | Field |
| --- | --- | --- |
| 0 | 1 | Message version |
| 1 | 2 | Vendor ID |
| 3 | 4 | Product ID |
| 7 | 2 | Vendor ID source |
| 9 | 1 | Case color |
| 10 | 2 | Low 16 bits of case version |
| 12 | 1 | Name length `N` |
| 13 | `N + 1` | Name bytes followed by NUL |
| `E` | 6 | Case version bits 16–63 |
| `E + 6` | 4 | Reserved value |

The two version fragments form one uint64 LE value. The native extension is
present when the message version exceeds 1; NTPods accepts only the captured
version-3, 25-byte form and applies its own NUL and UTF-8 validation.

For `4C`, payload offsets below start after the two-byte length. Tahoe 25G241's
`AACPClient::adaptiveVolumeMessageRecvEvent` supplies the field names and layout:

| Subtype | Exact payload | Meaning |
| --- | --- | --- |
| `04` | `04 mode version` | DRC mode and version, both raw uint8 values |
| `01` | `01 media telephony voiceCommand rampUp rampDown` | Three raw uint8 targets, then two uint16 LE ramp durations in milliseconds |

Pro 3 / 9A348 captures contain `04 02 00` and `04 3A 01` at A1211/1215 and
B1426/1429 in the [case capture](MACOS-CASE-CAPTURE-2026-10-12.md).
Subtype `01` is native-code evidence only; no captured report confirms it yet.
Unknown subtypes or shapes remain opaque. Mode `3A` is not a boolean or a known
user setting. These reports are separate from Personalized Volume (`09/26`)
and Adaptive strength (`09/2E`); no writable mapping is established.

The receive path recognizes `0x2B`, `0x44`, and `0x52` for route diagnostics
but does not decode their state. Their names in `route_opcode_name` are working
labels inherited from research. A `0x2E` list contains six-byte big-endian host
addresses followed by two flag bytes. The flags are not a reliable device type:
the same PC has reported several values. The parser rejects malformed or partial
lists without replacing the previous host state. Routine route logs show opcode,
length and safe counts without host identities or packet payloads.

Metadata parsing requires eleven NUL-terminated UTF-8 strings after the five-byte
information prefix. NTPods uses string 1 for model and string 3 for serial
(indices start at zero). For firmware it prefers a public version in string 10,
then string 4, then a recognized packed release value in string 10. The packed
conversion is inferred: major `value >> 20`, letter `A + ((value >> 16) & 15)`,
build `(value >> 4) & 4095`, with a zero low nibble. Unknown/internal versions do
not grant firmware-dependent capabilities. Trailing identity data is unparsed.
Do not publish names, serials or host addresses from a capture.

## Captured host clock: AACP 0x1B

Tahoe 25G241 sends this wall-clock message on the captured Pro 3 / 9A348
connections. NTPods sends it once per control connection after identifying a
hearing-capable model, using UTC for both numeric and text fields. Offsets include the complete
`04 00 04 00 1B 00` header:

| Offset | Field |
| --- | --- |
| 6 | Unix seconds, uint32 LE |
| 10 | Microseconds within the second, uint32 LE; less than 1,000,000 |
| 14 | UTF-8 date-text byte length `N`, uint16 LE |
| 16 | Exactly `N` text bytes; no trailing NUL |

The complete length is `16 + N`. Apple's sender uses `gettimeofday` and
formats the same whole second as RFC3339 text in the host's local time zone.
An offset such as `+02:00` belongs to that text representation; it is not added
to the numeric seconds. A separate periodic sender supplies no text (`N = 0`,
16 bytes). Preserve both forms when present. The pinned [Wireshark decoder](https://github.com/pabloaul/apple-wireshark/blob/2e9822247195fad2f79c729010308c33a29546ea/plugins/aacp/aacp_parse_message.lua#L150-L159)
displays the fractional field as nanoseconds, so its displayed fraction is
1,000 times too small. Decode that field as microseconds.

This is calendar time, separate from audio sample timestamps. The inspected sender
checks local submission status, with no identified clock-specific acknowledgment
or device-clock readback.

Pro 3 / 9A348 in-case comparisons sent current numeric UTC with equivalent
RFC3339 text, then requested a self-check through `A4120004`. With `+02:00`
text, one ear's new measurement date was 7,223 seconds after the clock send;
with `+00:00` text it was 29 seconds after. Both followed audible case sweeps;
cached dates stayed unchanged immediately after the messages. This strongly
supports a formatted-time offset affecting new diagnostic dates. The receiver
implementation is unverified, and neither run established a new successful
measurement in the other ear. Keep raw dates; do not apply a blanket correction
or infer a completed check from an ATT acknowledgment.

## Setting values and confirmation

The packet is `04 00 04 00 09 00 id v0 v1 v2 v3`, with the four value bytes in
little-endian order. This table is the allowlist in
[`valid_control`](../windows/daemon/src/device_settings.rs), not a promise that
every model supports every ID.

| ID | Setting | Accepted value and wire interpretation |
| --- | --- | --- |
| `0x01` | Microphone side | `0` Automatic, `1` Right, `2` Left |
| `0x0A` | Ear detection | `2` enabled, `1` disabled; opposite to the other boolean controls |
| `0x0D` | Listening mode | `1` Off, `2` ANC, `3` Transparency, `4` Adaptive |
| `0x16` | Press-and-hold action | `v0` right, `v1` left; each `1` noise control or `5` assistant; `v2=v3=0` |
| `0x17` | Press speed | `0` Default, `1` Slower, `2` Slowest |
| `0x18` | Hold duration | `0` Default, `1` Slower, `2` Slowest (current UI labels) |
| `0x1A` | Stem listening-mode cycle | Mask: Off `1`, ANC `2`, Transparency `4`, Adaptive `8`; at least two bits set |
| `0x1B` | ANC with one bud | `1` enabled, `2` disabled |
| `0x1C` | Max crown direction | `1` reversed, `2` default |
| `0x1F` | Tone volume | `v0=0..100`, `v1=0x50`, `v2=v3=0` |
| `0x20` | Connect automatically | `1` enabled, `2` disabled; device preference, not the Windows reconnect switch |
| `0x23` | Volume-swipe duration | `0` Default, `1` Longer, `2` Longest |
| `0x24` | Call gesture layout | `00 03 00 00` default, `00 02 00 00` swapped; no Windows call integration implied |
| `0x25` | Volume swipe | `1` enabled, `2` disabled |
| `0x26` | Personalized Volume | `1` enabled, `2` disabled |
| `0x28` | Conversation Awareness | `1` enabled, `2` disabled |
| `0x2E` | Adaptive strength | `0..100`; Android reference maps 0 to more outside noise, 100 to less |
| `0x31` | Case sounds | `1` enabled, `2` disabled |
| `0x34` | Allow Off | `1` enabled, `2` disabled |
| `0x35` | Sleep detection | `1` enabled, `2` disabled |
| `0x36` | Allow automatic connection | `1` allow, `2` disallow; distinct ID from `0x20` |
| `0x3B` | Optimized charging | `1` enabled, `2` disabled; upstream name is Dynamic End of Charge |
| `0x40` | Case tone volume | `0..100` |

Ear detection uses reversed boolean values. Apple's
[setter](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/usr/sbin/bluetoothd/bluetoothd_50.mm#L931-L1020)
sends `0x0A=2` when enabled and `1` when disabled, alongside a notification-filter
change. Windows ear-detection behavior still needs a controlled hardware check.
Wear reports (`0x06`) and NTPods' automatic-pause
preference are separate from that configuration value.

Outgoing hold-action commands (`0x16`) carry current right/left actions in the
low two bytes and zeroes in the high two. Apple's
[receive path](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/usr/sbin/bluetoothd/bluetoothd_50.mm#L7076-L7104)
also reports the previous right/left actions in the high bytes. NTPods validates
and retains only the current low pair; previous-action bytes never enter a new
command. This incoming form has not been captured on Windows.

Model allowlists and firmware checks in
[device_models.rs](../windows/daemon/src/device_models.rs) apply in addition to
value validation. On known catalog models, controls `01`, `0A`, `16`, `1A`,
`23`, `31` and `40` accept an explicit choice without a current device report.
Apple's getters for several of these preferences read its host cache; no wire
query for their current values has been verified. NTPods keeps the last
successfully sent choice separate from device reports, saves it per paired
identity, and replaces its displayed choice with any fresh valid report.
Loading a saved choice never sends it to the AirPods. A complete left/right
hold-action pair is required before writing `16`; an unknown half is never
filled with a default. Other settings still require their existing reports.
The Pro 3 Allow Off exception permits a silent write and saves
the choice per paired identity. Unknown models get only a conservative subset
of reported controls; they do not inherit Pro 3 EQ, microphone or sensor support.
Adaptive requires public firmware major 6+, sleep detection 8+, and head tracking
a recognized public firmware version. Profiles with an Allow Off option require
a report, a saved Pro 3 request, an already reported Off mode, or legacy firmware
major 1–6. Other listening profiles permit Off unless the device reports it disabled.

`PendingSettings` keeps reports separate from requests. Most writes wait up to ten
seconds for a matching report, starting after the transport write completes.
An explicit choice for an initially unreported preference is shown as sent,
without a false confirmation timeout.
Reports arriving during a write remain authoritative, and connection changes
invalidate old write completions. A timeout means unconfirmed, not rejected.

Adaptive strength and Pro 3 Allow Off can apply without an immediate echo. Their
successfully sent values are stored as requests, without inventing a report.
Adaptive strength requires a confirmed Adaptive listening mode. NTPods sends the
full 0–100 range. Strength changes were audible on Pro 3/9A348, but reconnect
reports were coarse (0, 50, 100); these do not establish three audible levels.
Keep requested values and device reports separate in capture comparisons.

EQ mode `1` is recommended sound; mode `2` selects custom low/mid/high values,
each `0..100`. The implementation requires a matching EQ-capable profile and
public firmware major at least 9. This is opcode `0x63`; the older `0x53`
[PME format](#pme-record-aacp-0x53) is separate from the app's EQ writer.

## Ear and conversation events

Ear reports contain primary/secondary states, not fixed left/right positions:
`0` in ear, `1` out, `2` in case, `3` disconnected, `4` transitional. Unknown values
are rejected. Transitional values retain the last stable in-ear state. The app
uses these events for automatic pause and microphone handoff; it resumes only a
media session it paused itself. These host actions are separate from device
setting `0x0A`.

LibrePods maps the first body byte of role message `0x08` as 1=left primary,
2=right primary. The [Tahoe case captures](MACOS-CASE-CAPTURE-2026-10-12.md)
contain left-primary reports, but do not validate every role flag. NTPods parses
the four-byte body and preserves unknown values. Its pause and microphone
policies still work on both wear slots without assigning physical sides.

Conversation event `0x4B` is separate from the enabled setting `0x09/0x28`.
Conversation Awareness may apply without echoing the setting. NTPods retains a
successfully sent choice for the current connection, separately from its last
report, so the switch does not revert after ten seconds. A fresh setting report
overrides that choice; a reconnect clears it. Speech events do not confirm the
setting or change the switch.
`parse_conversational_awareness` extracts byte 9; host ducking handles speech and
recovery stages in [volume.rs](../windows/daemon/src/volume.rs). Status `1` starts
ducking to the configured cap, `2` uses 60% of that cap, `3` uses the cap again,
and `4`/`6`/`7`/`8`/`9` restore the original volume. Unknown values are ignored;
`2` and `3` do not start a new duck. User volume changes take precedence over
restoration. Capturing through the high-quality mic suppresses host volume
ducking. A mode change or volume change alone does not prove the enable setting.

## Battery over AAP and BLE

AAP battery packets have `04 00 04 00 04 00 count`, then `count` five-byte records:
`component 01 percent status 01`. Components are `0x01` headset, `0x02` right,
`0x04` left, `0x08` case. Current parsing reads the component, percentage and
status; it does not require the two observed `01` bytes.

Percentages `0..100` are valid. `0xFF` or status `0x04` means unavailable. Status
`0x01` or `0x05` means charging when a percentage is present. Unknown components
are ignored; invalid percentages or an incomplete counted record reject the
packet. An omitted component leaves its prior state alone. An explicitly absent
component clears live/present/charging state but retains its last percentage and
original timestamp. This is how removing a bud can retain the known case level.

BLE decoding lives in [battery_ble.rs](../windows/daemon/src/battery_ble.rs);
[le.rs](../windows/daemon/src/le.rs) runs a passive Windows advertisement watcher.
It accepts Apple manufacturer data (company `0x004C`) from random addresses.
The manufacturer bytes below exclude the company ID, as returned by WinRT.

1. `0x31` supplies exactly 47 bytes: the six-byte AAP prefix, count `2`, then two
   20-byte records `type 00 10 00 key[16]`. Type `1` is the identity resolving key
   (IRK), type `4` the advertising encryption key. Both must occur once and be
   nonzero. Keys come only from the selected pair's AAP session.
2. Resolve the advertiser's private address before decrypting. Require address
   bits 47..46 to equal binary `01`. In big-endian six-byte address order, use
   the first three bytes as `prand`, the last three as the expected hash. Reverse
   the received IRK, AES-128-ECB encrypt `13 zero bytes || prand`, and compare the
   last three output bytes with that hash (`Keys::matches_address`).
3. Parse manufacturer type/length records and require one proximity record:
   type `0x07`, length `0x19`, total 27 bytes. Record byte 2 must be `1`.
   Bytes 3–4 are the model ID in the code's big-endian comparison order. The
   accepted set is explicit in `Keys::decode`; unknown IDs are rejected.
4. Decrypt record bytes 11–26 as one AES-128-ECB block using the encryption key.
   Decrypted bytes 1, 2 and 3 are primary, secondary and case battery slots.
   Each slot has a seven-bit percentage and charging in bit 7; level 127 is
   unavailable, levels 101–126 reject the report.
5. Record byte 5 bit `0x20` says the primary slot is left. Bit `0x04` indicates
   both buds in the case. A lid indication is accepted only if byte 5 has either
   bit in `0x44`; then byte 8 bit `0x08` means closed. Without those flags, lid
   state remains unknown.

For the two recognized Max model IDs (`0x0A20`, `0x1F20`), the first available
decrypted slot supplies one headset battery. NTPods does not infer Max wear or
Smart Case state from these flags. AES decryption and address resolution provide
identity checks; this format has no implemented replay counter or authenticated
payload check. A received packet's timestamp is the host's reception time.

Keys are DPAPI-protected for the current Windows user and stored per paired
identity. `Keys` deliberately has no Debug/Serialize implementation. Captures
containing the key response, serials or host addresses must stay private; replace
them with synthetic fixtures before contributing examples.

Public type `0x07` model/lid fields can invite pairing before keys exist; they
do not establish a unique identity or authorize audio. Reports verified as
belonging to the selected, still-paired Windows device are excluded before
changing unpaired invitation state. Removing its Windows bond permits
invitations again even when cached keys remain. Verified paired-lid events
follow a separate path.

### Freshness and connection policy

Battery cache values last for the daemon session and remain tied to the selected
pair. Each component keeps its own percentage timestamp, presence and live flag.
Reconnecting invalidates freshness but preserves percentages. A buds-only report
cannot renew the case's age, and stale charging indicators disappear.

The BLE cache can restore a fresh report when AAP is disconnected or stale. It
does not replace newer AAP data or a healthy connected AAP session. It can restore
the same BLE timestamp after a brief reconnect invalidated its freshness; this
does not reset its age. Reports expire after 90 seconds. Pro 3 closed-case reports
have arrived about a minute apart on the tested AX200 setup, so a ten-second
timeout would be too short. No later packet means cached/disconnected state,
not a request to wake the case.

Scanning while connected requires the saved opt-in or an active Find nearby
session; capture or an active microphone uplink always stops scanning.
Verified battery advertisements keep a closed case's battery
visible without reconnecting audio. Nearby and case-open reports never request
an audio connection: availability does not establish which host the user wants.
Connect, Repair, audio rebuild and pairing completion explicitly request audio;
an existing Windows link may resume controls only. Custom AVDTP playback requires
that explicit intent and stops when it is retired. A lost link releases the
completed request even when PC playback or capture continues. After startup or a
completed link loss, custom playback currently requires Connect because its
driver exposes no verified peer-initiated audio-return signal. Windows itself
can still reconnect its paired Bluetooth profiles. No advertisement can prove whether the case is currently
charging if its case slot is absent.

### Find nearby

[Find nearby](../windows/daemon/src/nearby_finder.rs) uses only advertisements whose
rotating address resolves with the selected pair's stored IRK. It accepts RSSI
without requiring a battery payload and rejects anonymous `0x12` Find My reports.
Signal uses a seven-sample median followed by exponential smoothing; trend
compares five smoothed readings with a 3 dB threshold. The UI retains the last
signal and its age when reports stop. Signal strength does not establish distance,
direction or which part of the pair transmitted.

Start and Stop require the selected device ID. The command-pipe owner holds the
session; an old client's Stop or disconnect cannot stop a newer session. Closing
or collapsing the view, changing devices, leaving the page or losing the daemon
ends the UI's session. Starting the finder temporarily permits connected scanning
without changing the saved preference. Microphone capture or uplink pauses it.
The finder never requests audio and suspends automatic case return while open.
Outside the finder, a fresh report whose rotating address resolves with the
selected pair's IRK can exercise one remembered
[case return](WINDOWS-FEATURES.md#automatic-return). This is local connection
policy, not a decoded Apple audio-ownership message.

Apple's supported [Find My setup](https://support.apple.com/en-lamr/guide/airpods/dev8e8b93d71/web)
requires initial pairing with an iPhone or iPad signed in to the Apple Account;
the enrolled pair can then appear and play sounds in Find My on a same-account
Mac. Mac-only enrollment is unproven. NTPods implements local signal finding;
Apple-account enrollment, cloud location, case ringing and UWB navigation remain
unimplemented.

The pinned [apple-wireshark decoder](https://github.com/pabloaul/apple-wireshark/tree/2e9822247195fad2f79c729010308c33a29546ea/plugins/cldurian)
labels its ClDurian command table `airtag_tasks`. Magnet's `0x91` dispatch and
AirTag sound entries do not establish AirPods sound commands. AirPods-specific
captures must verify the transport, authentication and target before adding a
case-ringing writer.

### Nearby charging-case information (DULT)

The case has a separate BLE GATT connection. NTPods reads its DULT protocol
version, capabilities and firmware value through service
`15190001-12F4-C226-88ED-2AC5579F2A85`, characteristic
`8E0C0001-1D68-FB92-BF61-48377421680E`. This is the Posh non-owner route in
iOS AccessoryInteraction, distinct from the Hawkeye `FD44` configuration route
and its Accessory Information service.

| Request | Response | Operand |
| --- | --- | --- |
| `07 00` | `07 08` | Protocol implementation version |
| `08 00` | `08 08` | Capabilities |
| `0A 00` | `0A 08` | Firmware value |

Opcodes are uint16 little-endian, with no extra header. Each accepted response
is exactly six bytes: opcode followed by uint32 little-endian data. Generic
Command Response `02 03` is not a metadata response, even with status zero.
The protocol and standard capability meanings follow the
[DULT draft, revision 03](https://datatracker.ietf.org/doc/html/draft-ledvina-apple-google-unwanted-trackers-03).
Capability bits 0–3 denote sound, motion detector, NFC identifier and BLE identifier;
unassigned bits remain raw. A sound capability does not prove a sound command works.

On the tested Pro 3 case with Apple PnP product `2029`, AX200 and Windows 11,
all three requests returned typed indications without a BLE bond. The user
reported that the pair was not registered in Find My. Values were protocol
`0x00010000` (1.0.0), capabilities `0x0000003B`, firmware `0x005A8600`.
These are uncached Windows API observations, not an HCI capture.
The firmware is shown raw; it is not the buds' marketing firmware or an AIS
Find My version. AIS was absent from this case's returned service inventory.

The bounded reader selects one fresh connectable candidate, verifies uncached
Apple PnP identity and refuses ambiguity or an existing BLE bond. It temporarily
enables indications, then attempts to restore the original notification setting
and checks readback, including after cancellation. If restoration cannot be
confirmed, the app retains a warning and reports it before quitting.
It does not pair, remove bonds, send
sound commands or open audio. Metadata is session-only and labelled nearby:
model identity alone cannot bind a case to the selected Classic pair. An optional
read after successful bud pairing does not delay audio or change pairing success.
Permanent Windows case bonding and subsequent encrypted reconnection remain
unresolved; this reader does not implement them.

A separate DULT sound experiment sent `00 03` (Start), then `01 03` (Stop)
after two seconds on the same unpaired connection. Both received ATT success;
only Stop returned a typed Command Response with status zero. There was no
Start response or Sound Completed event, and the user heard no sound.
Notification restoration was verified. NTPods has no case-ringing control;
the reported sound capability and successful Stop response do not establish
working Start/Stop behavior.

### Case sound and bonding limits

The iOS 27.0.1 (24A446) Hawkeye configuration writer uses `01 00 02` for Start
and `01 01 02` for Stop on service `FD44`, characteristic
`4F860002-943B-49EF-BED4-2F730304427A`. These are three-byte values: prefix `01`
and a uint16 LE opcode. They are distinct from the two-byte DULT commands above.
Its configuration response is seven bytes: `01 0B 02`, acknowledged opcode
uint16 LE, then status uint16 LE; `01 0D 02` is a separate completion event.
The native non-owner characteristic is `4F860003-943B-49EF-BED4-2F730304427A`,
with `01 00 03` / `01 01 03` sound values. These native layouts do not prove
that the tested case accepts either sound route.

On the user-identified Pro 3 case, Apple PnP `2029`, Windows 11 / AX200,
indication subscription before BLE pairing failed with ATT `0x05`
(Insufficient Authentication); no sound command was sent in that check.
Retaining the same connection through ordinary ConfirmOnly pairing allowed
encryption, subscription and both configuration sound writes. The BTHPORT trace
confirmed Encryption Change success with encryption enabled, ATT success for
Start/Stop, and notification-setting restoration. No protocol acknowledgment
or completion was decoded, and the listener heard no sound. A separate API-level
repeat after the pairing light stopped also produced no sound; that repeat
has no HCI capture.

A fresh bonded reconnect failed before GATT discovery. HCI Encryption Change
returned status `0x13` with encryption disabled, followed by disconnection
without an ATT exchange. This localizes the failure to connection/encryption
setup; it does not identify a missing key or establish a Find My registration
requirement. The retained-session trace references are records 1131
(encryption), 1262/1264 (Start), 1298/1302 (Stop), and 1365–1373 (restoration);
the separate reconnect uses records 194–198. Raw traces remain private.

The native Hawkeye hello path includes a separated-key latch, opcode `0x0206`.
Its activation and firmware requirements remain unresolved. Posh/DULT discovery
has a separate subscription path and does not run that Hawkeye latch; finding
both services does not establish a shared initialization recipe. The inspected
iOS `ClassicPlaySoundPayload.case` path uses Find My server/push handling and
does not establish a local AACP case-sound command. NTPods implements none of
these initialization or sound writers. The next prerequisite is a verified
initialization/authorization sequence and a working Start, Stop and completion
exchange on the identified case.

The native source is AccessoryInteraction UUID
`A7BDB20B-24BB-31B5-8C52-C204DE92D00D`; `startSoundHawkeyeTask` at `0x236a72d68`
and `sendHelloSequenceWithConfiguration:` at `0x236ac06a0` identify the separate
sound and initialization boundaries.

## Sensors

RTBuddy packets begin `04 00 04 00 17 00 00 00 10 00`, then a little-endian
16-bit payload length and protobuf-like fields. Both decoders reassemble split
frames and bound payloads to 16 KiB. [rtbuddy.rs](../windows/daemon/src/rtbuddy.rs)
parses descriptor announcements with bounded field counts and dictionary lengths.
Parse nested fields; sequence varints and wrappers change packet offsets.

Head tracking uses [head_tracking.rs](../windows/daemon/src/head_tracking.rs).
A recognized device descriptor selects the native path described below. Without
one, the firmware fallback sends field 1 sequence and field 8 ServiceSettings:
field 1 service, field 2 value `2`, field 3 five bytes `01 period_us_LE32`.
Period 40,000 µs starts reporting; zero stops it. Public firmware major 8+ uses
service `16`, older recognized firmware service `14`. A received sample requires
top-level log type (field 2) `1` or `3`, command field `7`, the requested service,
and exactly 58 payload bytes. Signed little-endian 16-bit values at record offsets
28 and 30 become horizontal and vertical graph samples. They are sensor units;
NTPods has no calibrated angles, quaternion or spatial-audio pose.

Head tracking stops or suspends on user stop, owning UI client close, ear removal,
connection loss or a five-second sample timeout. The fallback publishes at most 25 samples
per second. Service 14's shorter activity records do not count as head movement.

The [native macOS comparison](MACOS-CASE-CAPTURE-2026-10-12.md#native-head-tracking-comparison)
uses service 16. Start writes feature report `04 01 00 00 00` (Maximum FIFO
Events = 1), then `01 20 4E 00 00` (interval 20,000). Stop writes FIFO value
10, then interval zero. Reports arrive at about 50 Hz:

| Source | Envelope | Report data |
| --- | --- | --- |
| macOS | Top-level field 3: two-byte service prefix + 56 bytes; log type 5 adds a two-byte trailer after the protobuf | 56 bytes; also seen in a final field-7 report |
| Windows Pro 3 / 9A348 | 60-byte packed field: two-byte service prefix + 58 bytes; no log-5 trailer | A3063 primary: log type 1; A3064 primary: log type 3 |

NTPods accepts these forms after matching the announced `devmotion6` HID layout.
It strips the service prefix and log-5 trailer before reading raw channels at
report offsets 28 and 30. The channels remain uncalibrated. Only that descriptor
uses the 50 Hz/FIFO sequence; other firmware keeps the 25 Hz commands. The FIFO
setting's effect on latency remains unmeasured.

Heart rate uses [hr.rs](../windows/daemon/src/hr.rs), `aap::hr_stream`, and
`hr_retry_campaign` in [main.rs](../windows/daemon/src/main.rs). The runtime sends
the AACP 1.3 CONNECT/CAPABILITIES sequence for services 0 and 4, enables control
`0x30=1`, then requests 1,000,000 µs intervals on services `84` and `19`. Unlike the
head-tracking writer, `hr_stream` includes top-level field 2=`2` and a seven-bit
sequence. Disabling requests interval zero on both services. Existing descriptor
and raw PPG packet builders remain research helpers. Normal startup requests
sensor descriptors once per connection.

The HR decoder accepts live log types `1`/`3` and bounded nested command
envelopes. Before descriptor mapping, it accepts report services `19`/`20`/`84`;
once mapped, it accepts only the advertised HeartRate service. If that mapping
is withdrawn, it waits for a new descriptor instead of using the legacy list.
An accepted record is exactly 18 bytes: byte 1
is BPM (30–220), byte 2 confidence (at least `0x80`), and bytes 15–17 must match
one of `10 00 00`, `10 00 80`, `20 00 00`, `20 80 00`, `20 02 80`, `20 82 80`.
Other bytes remain uninterpreted. This filtering is based on observed records;
it does not validate medical accuracy. Startup ACKs do not count as readings.
The runtime makes at most four attempts, waiting up to 12 seconds for a reading
per attempt. A displayed reading expires after ten seconds without a new sample.
Heart-rate availability on a profile is not proof of a working
session or of the correct enable sequence on every firmware.

The [Pro 3/9A348 descriptor capture](MACOS-CASE-CAPTURE-2026-10-12.md#sensor-descriptions)
identifies services 14=`activity`, 16=`devmotion6`, 18=`SPL0`, 19=`HostLibHID`
and 20=`HeartRate` from device-supplied dictionaries. This firmware differs
from fixed service labels in the community dissector. Its HeartRate HID
descriptor confirms the 18-byte report and BPM at byte 1; the confidence and
status interpretations above still depend on observed records. SPL0's report
framing is known, but its vendor-defined fields have no established decibel
scale. A descriptor is not a live sample. NTPods recognizes the captured
`devmotion6`, `HeartRate` and `SPL0` dictionary/layout pairs to discover report
service IDs; heart-rate activation still uses the separately tested IDs 84/19.
SPL0 recognition does not start a stream or expose a sound-level reading.

Descriptor restarts or mapping changes clear partial sensor data and queue
recovery of user-enabled streams on the existing channel. Disabled sensors stay
disabled. NTPods retires changed motion addresses without writing to them; a
withdrawn mapping waits for a new descriptor. Firmware fallback applies only when
no native mapping has been discovered. The sensor worker sends queued refreshes under the control
locks; newer mappings replace queued older ones. Heart-rate refreshes retain the
user's request generation and cannot restart an active enable campaign from its
own descriptor announcements.

An [active SPL0 probe](MACOS-CASE-CAPTURE-2026-10-12.md#active-spl0-probes)
read and wrote feature report 1 through Tahoe's native IOHID API. The device
returned five bytes despite its 13-byte feature capacity. Feature reads use
top-level field 10 with nested service/type/report-ID fields 1/2/3; field 11
returns the service in field 1 and report bytes in field 3. Writing
`01 40 42 0F 00` through field 8 was acknowledged and read back unchanged,
but produced no samples during a 12-second listening window with both buds
worn. Restoring `01 00 00 00 00` was confirmed. This setting alone did not
start SPL0. Additional enable conditions, configuration and firmware support
remain unresolved.

The iOS 27.0.1 / 24A446 HearingTest consumer matches SPL0's vendor collection
`0xFF00:0x60` and 57-byte input report 1. It reads eight little-endian Float32
values at report offsets 9–40, applies `10 * log10(value)`, then interpolates
adjacent log-power values into 16 bands. The physical reference, band frequencies
and live acquisition conditions remain unverified. Missing, stale or nonpositive
values cannot establish acceptable ambient noise.

## ATT settings

[hearing.rs](../windows/daemon/src/hearing.rs) and
[transparency.rs](../windows/daemon/src/transparency.rs) share
[UUID discovery](../windows/daemon/src/att_settings.rs) for hearing settings and
Custom Transparency. Loud Sound Reduction still uses its fixed upstream handle;
that assumption needs comparison on each model/firmware.

| Handle | Operation | Current format |
| --- | --- | --- |
| `0x001B` | Loud Sound Reduction | Read response `0B value`, value `0`/`1`; write `12 1B 00 value`, require `13`, then read back |
| Discovered (`0x002A` in captures) | Hearing-assistance settings | Read 104 value bytes; validate the header, both audiograms and per-ear controls; write the known V2 format using the advertised method and confirm by readback |
| `0x002B` in captures | Settings CCCD | macOS subscribes here; NTPods does not change hearing subscriptions |

Separate custom Transparency settings were identified in the
[macOS captures](MACOS-CASE-CAPTURE-2026-10-12.md#custom-transparency-and-accommodation)
at characteristic UUID `D5621CC1-F7AB-45DB-9403-9EAF744D5390`, handle `0x0018`
on the tested pair. [transparency.rs](../windows/daemon/src/transparency.rs)
discovers service `7798082B-B7B7-45A6-9933-563492EFE04E` and this characteristic by
UUID. Its experimental editor accepts exactly 104 bytes. A write includes the
confirmed settings from before the edit as `expected`; these must match both the
daemon's current settings and a fresh ATT read. Missing or changed settings
require another read before editing. The UI preserves this baseline while an
edit is unsent, even if another client changes the device.

Edits preserve both eight-band curves, own-voice bytes and every untouched
control. Readback must match all 104 written bytes. If it still returns the exact
original record, NTPods retries the read up to four times at 75 ms intervals.
Other mismatches or errors fail immediately; the write is never repeated.

The captured characteristic advertises Read, Write Without Response and Notify
(`0x16`), so NTPods sends a Write Command (`0x52`) without
waiting for an ACK. Characteristics advertising Write use a Write Request (`0x12`)
and its ACK instead. It exposes the float32 enable flag at offset 0 and
per-ear amplification/tone (−1–1), Conversation Boost (0/1), and ambient-noise
reduction (0–1) at the offsets below. It sends no settings when opened or at startup.
The Pro family may attempt discovery; older 100-byte layouts remain unsupported.
Persistence across reconnects remains unverified; see
[hardware coverage](WINDOWS-VALIDATION.md#controls-batteries-and-sensors).

Two [public listening-mode captures](https://github.com/BorgoGustavo/airpods-ctl/tree/b42f4e8a944d39fdd9fa25533ae262bf647ddb3b/docs/captures)
(`control-center-mode-changes.pklg` and `mode-transitions-2026-05-28.pklg`,
author-reported Pro 2 / 8B39) contain six ATT reads of handle `0x0018` after
mode changes. All six return identical 104-byte values with nonzero eight-band
curves matching this layout. The files omit UUID discovery, so they do not
establish the characteristic identity or a fitted hearing profile by themselves.
The request/response traffic is not motion telemetry and gives no evidence
that mode changes alter these settings. The preservation regression uses
distinct synthetic left/right curves, including signed zero, to check that
editing exposed controls leaves all curve and own-voice bytes unchanged.

Pro 3 / 9A348 captures from Tahoe 26.7.1 (25G241) on AX200 contain successful
104-byte reads of Custom Transparency and hearing settings. The four unenrolled
hearing values have identical zero per-ear curves. These captures contain no
hearing-settings writes. The pairing trace establishes:

- Encryption completed before ATT opened on BR/EDR PSM `0x001F`.
- Tahoe subscribed before reading each characteristic.
- Both requests and the hearing response preceded AACP Connect.
- Both reads succeeded before the buds' `0x2C`/`0x33` reports, without either
  hearing-enable control from the host.

Hearing settings use characteristic UUID `A4120005-95C5-4D6F-9098-0F0B41457E0A`.
The captured properties are `0x16` (Read, Write Without Response, Notify).
NTPods discovers the value handle and uses Write Command (`0x52`) when only
Write Without Response is advertised. If Write is advertised, it uses Write
Request (`0x12`) and requires the exact `0x13` response. A characteristic that
advertises only Read remains readable. Each write requires matching readback;
there is no notification consumer or fixed CCCD write.

The four unenrolled hearing reads across connection, case and wearing captures
return header `02 00 60 00`. After iPhone Hearing Test setup, Pro 3 / 9A348
returned `02 02 60 00` with nonzero per-ear curves while Hearing Aid and Media
Assist were off. The stored value matched after refreshing the Windows control
connection. NTPods accepts exactly 104 bytes with header `02 00 60 00`,
`02 00 64 00`, `02 02 60 00` or `02 02 64 00`. The `02 00` forms stay read-only;
the `02 02` forms permit edits when the characteristic advertises writes.
Other headers and lengths are rejected.

Apple's iOS 26.1 [writer](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/PrivateFrameworks/HearingModeService_Private.framework/HearingModeService_Private/HMDeviceManager.mm#L1630-L1652)
builds a 104-byte V2 record. Its [prefix serializer](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/PrivateFrameworks/HearingModeService_Private.framework/HearingModeService_Private/HMDeviceManager.mm#L1755-L1803)
copies the existing prefix, keeps version 2, and sets the next fields to 2 and
100, producing `02 02 64 00`. NTPods uses that write header and preserves every
unmodified body byte. Readback must return `02 02 60 00` or `02 02 64 00` and
match all 100 body bytes exactly. If a read still returns the complete original
body, NTPods retries the read up to four times at 75 ms intervals without
resending the write. Any other mismatch fails. The `0x60`/`0x64` read variation
does not authorize other header changes.

The iOS 27.0.1 (24A446) native writer independently retains the 100-byte
prefix and 104-byte V2 record, with own voice at offset 100. Its receiver's
100-byte prefix read does not explain the fitted-read `0x0060` field.

A development preview on Pro 3 A3063 / 9A348 confirmed Amplification, Balance,
Tone, Conversation Boost and one left 250 Hz audiogram edit, restoring the full
original record after each. The listener reported audible changes in
Transparency, without identifying each control separately. See
[hardware coverage](WINDOWS-VALIDATION.md#controls-batteries-and-sensors) for
conditions, timing and remaining checks. Raw header normalization remains
unverified.

On Windows 11 with AX200 and Pro 3 A3064 / 9A348:

| Condition | Observed result |
| --- | --- |
| Without the Apple host identity | Both values returned ATT `0x0E` |
| Apple identity applied, Windows restarted, no re-pairing | Both reads succeeded |
| Custom Transparency enabled, per-ear noise reduction `0.125` | Audible change and matching readback; restoration preserved the separate hearing settings |
| NTPods 0.2.26 | Both restored values read successfully |

The Windows SDP response has not been captured.

In the Tahoe pairing capture, the buds requested the host's SDP Device ID before the
first successful read; Tahoe replied with Apple vendor `0x004C` and vendor source
`0x0001`. No hearing-enable command preceded that read.
[LibrePods main](https://github.com/librepods-org/librepods/blob/efccc4499b2c1fd2ac243e975ae1f42d8e828ffd/README.md#vendorid-spoofing)
requires Apple Device ID spoofing for these settings, and its
[connection code](https://github.com/librepods-org/librepods/blob/efccc4499b2c1fd2ac243e975ae1f42d8e828ffd/android/app/src/main/java/me/kavishdevar/librepods/services/AirPodsService.kt#L2659)
opens ATT only with `vendor_id_hook` enabled. NTPods setup configures Windows'
[Device ID record](https://learn.microsoft.com/en-us/windows-hardware/drivers/bluetooth/bluetooth-host-radio-support#how-to-change-the-device-id-profile-record-published-by-windows);
there is no app toggle.

Microsoft documents `DIDVendorID` and `DIDVendorIDSource` DWORD overrides under
`HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters`. These affect the
local host identity for all Bluetooth peers. Setup backs up each original
value's presence, type and contents for restoration on uninstall. Adding a
second SDP record does not establish which record the buds use.
The [LibrePods maintainer reports identity caching](https://github.com/librepods-org/librepods/issues/558#issuecomment-4360325275),
so a reconnect alone may not refresh it; Bluetooth restart or re-pairing may be
needed. ATT `0x0E` is a generic Unlikely Error, not a diagnosis of identity
rejection. NTPods already requests authenticated and encrypted ATT links.
The [host identity reference](BLUETOOTH-HOST-IDENTITY.md) describes installation,
recovery and the required Windows restart. Building alone changes no registry
values.

Tahoe subscribes before reading, but that sequence does not establish that
subscription is required. The
[GATT specification, section 3.3.3.3](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-62/out/en/host/generic-attribute-profile--gatt-.html)
defines CCCD state per client, shared across ATT bearers and persistent for bonded
devices. Closing a diagnostic socket therefore does not guarantee restoration
of an unknown previous subscription.

Hearing Aid uses control `0x2C`: its first value byte is enrollment and its
second is enabled (`1`/`2`). Mode changes preserve enrollment. Control `0x33`
is the shared gate: on while Hearing Aid or Media Assist is on. Disabling one
must preserve the other. Hearing Aid enable selects Transparency; Media Assist
keeps the current listening mode. These controls remain outside the generic
setting allowlist. Settings offsets relative to the characteristic value are:

| Offset | Field |
| --- | --- |
| 0–3 | Validated header; writes use `02 02 64 00`; see accepted read formats above |
| 4 / 52 | Left/right eight-band audiogram, eight little-endian float32 values |
| 36 / 84 | Left/right amplification |
| 40 / 88 | Tone |
| 44 / 92 | Conversation Boost, float `0`/`1` |
| 48 / 96 | Ambient-noise reduction |
| 100 | Own voice |

The eight audiogram slots are 250, 500, 1000, 2000, 3000, 4000, 6000 and
8000 Hz, in that order. iOS 27.0.1 (24A446)'s native
`HMHealthKitUtilities.frequencyBins` independently confirms this mapping.

The native audiogram importer reads per-ear thresholds in dB HL. It requires
250, 500, 1000, 2000, 4000 and 8000 Hz for both ears. If 3000 Hz is absent, it
averages the 2000/4000 Hz values; if 6000 Hz is absent, it averages 4000/8000 Hz.
Each ear is handled separately, using float32 values and arithmetic. Supplied
3000/6000 Hz thresholds are retained. The input selection prefers the first
available per-ear `clampedSensitivity`, falling back to `sensitivity`; that
property's numerical range has not been traced.

`updateHMSettingsStruct:fromAudiogram:` (`0x2808f6954`) stores these thresholds
directly in the profile. The traced construction/upload chain adds the scalar
adjustments and writes the record to `A4120005`; it does not calculate a filter
bank or compression prescription. The firmware's ambient fitting algorithm
remains unknown. NTPods still requires complete eight-band input and retains
its existing validation bounds.

Mode-only enables read the enrolled fitting first and verify its complete bytes
afterward; mode-only requests never write `A4120005` or notification subscriptions.
Fresh device reports confirm both switches. NTPods uses reports from the current
session when available; otherwise it refreshes the control connection while the
microphone is idle. Only the refresh owned by that pending mode request bypasses
the manual refresh cooldown. Enables also verify the unchanged fitting; disables
do not read it. Missing or inconsistent mode reports leave the result unconfirmed.
No dedicated mode-query command has been verified. Apple's host cache updates
after successful sends do not establish device confirmation. An explicit Off
retry retains its last confirmed raw template within the same connection;
reconnects discard that template.

Hearing assistance has an explicit read operation that sends no enable command
or notification subscription. Fitting edits require that successful read
in the current connection, complete eight-band curves for both ears and a known
AAP enabled state. Before a fitting edit's enable command or settings write,
the backend compares the caller's original baseline with the latest confirmed state, reads
again and rejects any mismatch. A refresh by another client cannot authorize an
older draft. It retains the original value across the enable sequence,
preserves untouched body bytes and requires exact body readback with one of the
two known fitted V2 headers. Discovery repeats after a new enable before choosing
the write handle and method.

The editor starts from those confirmed per-ear values. Scalar edits retain both
audiograms and untouched per-ear controls, including noise reduction and own
voice. Audiogram edits require an explicit Apply; tuning changes are coalesced
for 1.5 seconds. Empty or incomplete curves are rejected. Amplification accepts
−1.5–1.5, tone −1–1, thresholds 0–90 dB HL, and noise reduction and own voice 0–1.
The amplification slider uses `A = 2a - 1` for normalized position `a`; signed
balance `B` produces left gain `A - B/2` and right gain `A + B/2`. This matches
Apple's [gain conversion](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/PrivateFrameworks/HearingModeService_Private.framework/HearingModeService_Private/HMDeviceManager.mm#L1904-L1938),
also corroborated by Tahoe 25G241 and iOS 27.0.1 (24A446)'s native writers. Existing gains
remain unchanged until a gain slider is edited. Tone and Conversation Boost
apply to both ears; untouched asymmetric values remain intact.
The iOS native new-audiogram selection path resets normalized amplification,
balance, own voice and tone to 0.5, and beamformer/noise suppression to zero.
That policy applies to selecting a new audiogram; ordinary edits preserve
untouched settings. HearingTest's recovered B698/B788 stimulus-calibration
tables do not establish personal fitting or on-bud DSP coefficients.

The current native enrollment callbacks select an audiogram, request Hearing
Assist enrollment, enable Hearing Aid, then enable the media/voice settings.
They are not a complete fresh-enrollment procedure: earlier prerequisites and
error recovery remain unverified. The writer gets its version from the device
record; with no saved record and version zero, its fallback is V1. NTPods keeps
its provisioned-profile requirement rather than synthesizing a fitted V2 record.

Apple starts AHPS discovery before audiogram selection, subscribing to and
reading the settings, protection and diagnostic characteristics. Its record
decoder stores the received version before checking enrollment state; an
unenrolled V2 record can therefore supply the writer's version. This is a
source-derived path, not a verified fresh-enrollment procedure. The native
decoder's fixed-size copies do not establish safe handling of short records.

HealthKit availability also affects enrollment state. In
`_deviceUpdateRegionStatus:` (`0x2808FDDD8`), a Hearing Assist status change to
3 invokes `_deviceRescindHearingAssist:`. The underlying country, age and other
eligibility requirements remain unresolved; the UI's region analytics alone
do not describe that policy.

The iOS 27.0.1 findings use iPhone19,2 build 24A446, with native framework UUIDs
`43CA5DAE-B9BD-329B-9CA3-590BE7BAA8A6` (HearingModeService_Private),
`D0838E2A-D9A3-3F7F-8059-14BC86742EF6` (HearingModeService), and
`312AF164-11A5-363E-AEC3-523B6D1767C5` (HearingTest).
The enrollment caller is HearingModeUI UUID
`EA6FB41F-1E7B-39A7-80BA-5F20FD4E005E`, `enrollHearingAid` at `0x28096f398`.

Invalid values are rejected. Failed reads or writes require another read before
editing; a reconnect clears the saved baseline. The UI uses a session-bound editability
flag; missing flags from older backends default to read-only. Disable remains
available after a read failure. A failed new enable requests AAP-only disable while that request still
owns the connection; cleanup failure is reported. The editor keeps a recovery
Off action available until the AirPods report Off. Hearing reads and enable/edit
requests require an idle microphone and defer new microphone startup until they
finish. Disabling assistance remains available during recording.
Transactions share one lock. A failed ambiguous ATT transaction requires a new
connection before retry because late read responses and write ACKs lack handles.
Custom Transparency enable requires both Hearing Aid and Media Assist to be
confirmed off on hearing-capable models. The complete `02 00 00 00` payload is an
inactive, unenrolled state observed in both the Pro 3 macOS capture and Windows
test. An unset enabled byte is not accepted for other enrollment values.
Hearing-assistance enable first reads
Custom Transparency under the same lock and rejects an active setting. Reads
and disable requests remain available. A firmware that exposes ATT only after
hearing-assistance enable may fail this check; it needs a verified wake sequence.

The opt-in `NTPODS_GATT_PROBE` environment flag runs
[`gatt::probe`](../windows/daemon/src/gatt.rs) once per daemon process. This active
diagnostic enables hearing assistance with `0x2C`/`0x33`, exchanges ATT MTU 247,
requests primary services, and writes `01 00` to seven fixed CCCD handles
(`0x0008`, `0x000B`, `0x000E`, `0x0011`, `0x0019`, `0x0022`, `0x002B`). It then
collects raw replies and notifications for about 15 seconds. It blocks normal
AAP receive work during the probe and does not restore the hearing-assistance
settings or subscriptions afterward. Keep it disabled for baseline captures;
its raw diagnostic output also needs review before sharing.

### Hearing-test mode

The iOS 27.0.1 / 24A446 HearingTest code writes `01` to enable and `00` to
disable `A4120001-95C5-4D6F-9098-0F0B41457E0A`, using Write Without Response.
It separately reads the characteristic until the expected state appears;
completed writes alone do not prove the mode changed. Sustained operation sends
`02` to the same characteristic every second, followed by a state read.
States 0–4 are distinct native values; only 0/off and 1/enabled are verified
here. Intermediate values have no established labels.

The research-only `ProbeHearingTestMode` command discovers the UUID and write
property, requires an initial off state, then verifies `0 → 1 → 0`. This silent
cycle passed on Pro 3 / 9A348, with Hearing Aid and Media Assist off and playback
and microphone use paused. Playback and normal volume were confirmed afterward.
The command plays no tones and writes no hearing profile. Cancellation still
attempts same-session disable; a timeout or changed connection cannot certify
restoration. The normal build rejects this command.

`A4120003` carries a separate 12-byte Write Without Response record:
Float32 frequency in Hz at offset 0, Float32 original requested dB HL at offset
4 and UInt32 side at offset 8 (1 left, 2 right), all little-endian. The firmware
level is separate from the calibrated dBFS used to generate PCM. Native enable
also blocks host handoff before the mode write and starts its heartbeat only
after state 1. Cleanup stops the heartbeat before the disable write, then
releases handoff blocking. This sustained sequence has not been verified on
Windows. Neither tone metadata nor a successful mode check establishes acoustic
calibration.

The native category-volume request is 0.8125. Under the Bluetooth HAL's
software-volume policy, its 17-point curve maps that knob value to −10 dB;
the separate remote-volume path sends `trunc(127 × knob)`, or 103 for an
unchanged 0.8125. Route/category limits can alter the knob upstream. The active
HearingTest route's gain policy remains unverified, so neither a PCM multiplier
of 0.8125 nor combined local/remote attenuation reproduces calibration by
itself.

### Diagnostic and dosimetry records

These records belong to service `7798082B-B7B7-45A6-9933-563492EFE04E`.
The **Hardware checks** card reads saved results on Pro 2 and Pro 3. Refresh
discovers `A4120002` by UUID and reads its handle while the microphone is idle;
it sends no test, clock, subscription or hearing-mode writes. Replaced-session
results are discarded. Measurement and first-use dates remain device times,
separate from the time NTPods read them.

**Read device data** reads the raw test-mode byte, dosimetry sensitivity and
17-point volume curve in one bounded discovery. Missing characteristics remain
unavailable; malformed lengths and non-finite floats reject the bundle. It does
not write test mode or tone information, or convert the raw values into SPL.

**Run in-case check** is restricted to the verified Pro 3 / 9A348 combination.
It requires explicit confirmation, a recent report of both buds in the case,
and idle playback and microphone use. After a fresh baseline read, it sends
`31` once to the discovered write-request characteristic. A lost reply does
not cause another write. The app asks the user to close the case, let any sweeps
finish, then reopen it. It waits up to two minutes across same-pair reconnects
and compares each ear's nonzero measurement timestamp with its own baseline.
Both timestamps must change before it reports new results; an aborted
measurement also counts as new data, not a health pass. **Stop waiting** ends
host observation; no firmware cancellation command is established. This app
flow remains unverified on hardware.

The offline decoder also reads saved 71-byte records without connecting:

```powershell
cargo run --locked --manifest-path windows/daemon/Cargo.toml --example decode_hearing_diagnostics -- record.bin 2027
```

The optional final argument is the hexadecimal product ID. Without it, component
flags remain uninterpreted. The formats below come from captured traffic and pinned Apple-framework decompilations;
they are not a published protocol specification.

| Characteristic UUID | Value format |
| --- | --- |
| `A4120001-95C5-4D6F-9098-0F0B41457E0A` | Hearing-test mode: `01` enable, `00` disable, `02` heartbeat; separate state readback |
| `A4120002-95C5-4D6F-9098-0F0B41457E0A` | Hardware diagnostic status; observed version 8 is 71 bytes |
| `A4120003-95C5-4D6F-9098-0F0B41457E0A` | Tone information: Float32 frequency, Float32 level, UInt32 side; 12 bytes LE |
| `A4120004-95C5-4D6F-9098-0F0B41457E0A` | On-demand diagnostic request: ASCII `1` (`31`), Write Request |
| `82F6BC0A-1BCA-4873-AFC9-EC5890E3469A` | Dosimetry Sensitivity: one little-endian float32, 4 bytes |
| `D5F96AFA-2F2C-41BB-A7E6-F54ABE6235B4` | Dosimetry Volume Curve: 17 consecutive little-endian float32 values, 68 bytes; no header or count |

The [UUID table](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/Frameworks/CoreBluetooth.framework/CoreBluetooth/CBUUID.mm#L523-L528)
names the dosimetry fields. Apple's [audio reader](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/Audio/Plug-Ins/HAL/BTAudioHALPlugin.driver/BTHearingHealthObject.mm#L560-L654)
establishes their float layout. The [iOS 18.2 volume-map reader](https://github.com/EthanArbuckle/iPhone17-1_18.2_22C152_Restore/blob/e26ed4563f78871c59d2d96856756a65d62517e5/Library/Audio/Plug-Ins/HAL/VirtualAudio.plugin/VirtualAudio_4.m#L13582-L13688)
pairs the entries with equally spaced points across the audio device's reported
minimum/maximum volume range: 17 entries give 16 intervals. The ATT value does
not encode that range. Reference stimulus and weighting remain unverified;
these are not personal hearing thresholds or enough information for an absolute
SPL meter.

Apple [documents periodic quiet in-case sounds](https://support.apple.com/en-euro/guide/airpods/devd9aac5b42/web)
to help ensure microphone and speaker performance. In a controlled Pro 3 / 9A348
check, an on-demand request coincided with audible case sweeps and a fresh
left-ear diagnostic result. Whether Apple's periodic sounds use the same request,
or change firmware calibration, remains unverified.

The [diagnostic parser](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/PrivateFrameworks/HearingModeService.framework/HearingModeService/HMDeviceDiagnosticRecord.mm#L2112-L2235)
identifies the first 41 bytes; the [matching 18.2 reader](https://github.com/EthanArbuckle/iPhone17-1_18.2_22C152_Restore/blob/e26ed4563f78871c59d2d96856756a65d62517e5/System/Library/PrivateFrameworks/HearingModeService.framework/HMDeviceDiagnosticRecord.m#L1726-L1765)
confirms the packed field widths. Offsets are relative to the characteristic value:

| Offset | Field |
| --- | --- |
| 0 | Version byte |
| 1 / 9 | Left/right last diagnostic measurement time, uint64 LE Unix seconds |
| 17 / 21 | Left/right diagnostic result words, uint32 LE |
| 25–32 | ANC-loss score structure; individual score units remain unresolved |
| 33 / 37 | Left/right first-use time, uint32 LE Unix seconds |
| 41–70 | Opaque version-8 tail |

Read-only inspection of Tahoe 25G241's `HMDeviceDiagnosticRecord` confirms these
enum names and model gates:

| Raw value | General status, bits 0–2 | Measurement status, bits 3–5 |
| --- | --- | --- |
| 0 | `Pass` | `SuccessHarmonic` |
| 1 | `Fail` | `SuccessNonHarmonic` |
| 2 | `NoDataOutOfFactoryLessThanAYear` | `LidOpenAbort` |
| 3 | `NoUTPConnection` | `MotionAbort` |
| 4 | `NoDataOutOfFactoryMoreThanAYear` | `TempAbort` |
| 5 | Unmapped | `NoiseAbort` |
| 6 | Unmapped | `SystemAbort` |
| 7 | Unmapped | `NoStatus` |

Component flags cover distortion, frequency accuracy, speaker, vents and
microphones. Their raw `0`/`1` labels are `Pass`/`Fail`. NTPods exposes them only
for measurement status `0` or `1`, a nonzero measurement timestamp, and product
IDs `0x2014`, `0x2024` or Pro 3 `0x2027`. The same products pass Apple's
occlusion-support gate, but an occlusion assessment also needs accumulated
fault history and indication state. A cached failure flag does not establish a
current hardware fault. Device-clock alignment remains uncertain, so NTPods
preserves the reported seconds without applying timezone or age corrections.
A zero timestamp cannot produce a completed-check label.

Apple's [ANC-score reader](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/PrivateFrameworks/HearingModeService.framework/HearingModeService/HMDeviceDiagnosticRecord.mm#L3167-L3202)
assigns bytes `27/28` to full-loss left/right, `29/30` to clean-loss left/right,
and `31/32` to severe-loss left/right. Bytes `25/26`, score units and thresholds
remain unresolved. The offline decoder exposes these raw scores only for
product `0x201B` with a dated, completed measurement; Apple's version gate is
at least 4, while NTPods accepts only the observed version 8. Pro 3 does not
pass this product gate. Result bit 15 is retained without assigning an ANC-loss
name, and the app does not display the scores.

[Later source changes](https://github.com/blacktop/ipsw-diffs/blob/af456047befbab058df4c3d7c7094462ad4753d6/iOS/26_5_23F77_vs_27_0_24A5355q/DYLIBS/System/Library/PrivateFrameworks/HearingModeService.framework/HearingModeService.md#L85-L158)
name mileage, model version, test-start time, attempt hash and pre-check flags;
their offsets and widths in this tail are unverified. Diagnostic times must not
be labelled Hearing Test dates.

Apple's [on-demand request](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/PrivateFrameworks/HearingModeService_Private.framework/HearingModeService_Private/HMDeviceAHPSConnectionManager.mm#L1000-L1125)
writes `31` with response. The saved Tahoe pairing capture
`20261012T211811Z-048041f7` confirms handle `0x0028`, request frame 5473 and
ACK frame 5507. Its [completion callback](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/PrivateFrameworks/HearingModeService_Private.framework/HearingModeService_Private/HMDeviceAHPSConnectionManager.mm#L805-L861)
confirms the write only. Fresh per-ear diagnostic timestamps/results are needed
to establish a new measurement. The inspected host path does not establish
firmware requirements for lid state, playback or microphone use, or prove that
all internal calibration remains unchanged.

## PME record: AACP 0x53

NTPods retains complete subtype-2 reports for the Media Assist switch. It is separate from ATT hearing
settings and opcode `0x63` EQ. The [version-2 Apple writer](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/PrivateFrameworks/HearingModeService_Private.framework/HearingModeService_Private/HMDeviceManager.mm#L1046)
builds a 132-byte body after `04 00 04 00 53 00 84 00`:

| Body offset | Full packet offset | Field |
| --- | --- | --- |
| 0 | 8 | Version `2` |
| 1 | 9 | Enroll PME Voice |
| 2 | 10 | Enable PME Voice |
| 3 | 11 | Enable PME Media |
| 4 | 12 | Voice left: eight float32 LE values |
| 36 | 44 | Media left: eight float32 LE values |
| 68 | 76 | Voice right: eight float32 LE values |
| 100 | 108 | Media right: eight float32 LE values |

The writer uses `01` for enabled/enrolled and `02` for disabled. A zero input
configuration field preserves its existing byte; this does not establish a
meaning for received wire value `00`. Media Assist changes only the voice/media
enable bytes, using both on as Apple's initial enrollment default and both off when
disabled. Enrollment and all curve bytes are preserved; missing profiles are
never synthesized. The aggregate state requires `0x33` on and either enable
flag on. Apple's [configuration policy](https://github.com/EthanArbuckle/iPhone18-3_26.1_23B85_Restore/blob/90aa0cfe59d9682b4265e1354c8b19ec3c7823ab/System/Library/PrivateFrameworks/HearingModeService.framework/HearingModeService/HMDeviceConfigurations.mm#L1354-L1425)
keeps that shared gate on while either hearing mode needs it.

A separate `0x53` subtype-1 body `01 01`/`01 02` controls Apple's `PME On Buds`
audio-route property. NTPods does not write that gate; its activation policy
remains unresolved. On Windows with AX200 and an iPhone-fitted Pro 3 / 9A348,
the listener reported a music change with Media Assist on and Hearing Aid off.
That observation does not establish where processing runs or whether every
configuration needs the separate gate.
In iOS 27.0.1 (24A446), PersonalAudio's `lossArrayFromDictionary:`
(`0x29491116C`) reads dB HL loss keys in order 250, 500, 1000, 2000, 3000,
4000, 6000 and 10000 Hz. `onBudsMediaSettings` transfers four such arrays to
the PME writer. The native AHPS-to-PersonalAudio bridge feeds the eighth
AHPS value into the `10000` key directly; no 8000-to-10000 Hz resampling occurs
in that bridge. These are host-source assignments, not measured per-band
responses from the buds or evidence of a 24-band fitting transform.

PME and AHPS contain separate curves, with separate updates in Apple's host.
While shared Hearing Assist is active, the traced PersonalAudio bridge uses
the AHPS ear-loss arrays; otherwise it uses the separate stored media/voice
arrays. That host policy does not establish the buds' selection policy.
NTPods' audiogram editor writes AHPS only; Media Assist mode changes preserve
the stored PME curves. Synchronization after an AHPS edit remains unverified.
A coordinated curve update still needs device readback and restoration checks.

The PersonalAudio source is UUID `FC2D1EB8-2D5F-3C86-AC56-E49F730A76DD`;
its configuration bridge is `0x29490D570`. The separate host `hear` Audio Unit
in `libAudioDSP.dylib` (UUID `E5A8D405-69F6-3D20-A574-CC3AD83CE646`) calculates
gain targets by interpolating hearing-loss rows and frequencies, with separate
media and speech biases. These host DSP coefficients and bounds do not establish
the AirPods' ambient fitting algorithm or change NTPods' input limits.

## Evidence and comparison gaps

| Area | Evidence currently recorded |
| --- | --- |
| Pro 3, 9A348 controls | Listening-mode replies and audible Adaptive changes; silent Adaptive and Allow Off writes examined with reconnect readbacks |
| Pro 1, 6F21 controls | Off/Transparency/ANC, ear pause/resume and battery/case retention checked; press-speed writes verified after reconnect, without immediate echo |
| Closed-case BLE | Pro 3/AX200 received encrypted reports after audio disconnected, including roughly minute-spaced closed-lid reports; this does not establish all case sleep/charger behaviors |
| Head movement | Pro 3 graph followed turns/nods; stop and client-close cleared delivered events. Pro 1 service 14 produced no accepted samples and remains unresolved |
| HR, ATT and other profiles | Builders/parsers and upstream-derived mappings exist; do not infer a current hardware pass from the code or catalog alone |

The [validation record](WINDOWS-VALIDATION.md) retains dates and test scope.
[device-models.json](../windows/device-models.json) is the shared capability
catalog, not a tested-device list. Other model IDs, legacy firmware, Max sensor
services and fixed ATT handles need independent evidence.

For macOS captures, compare connection setup and subscription masks; settings
writes, echoes and reconnect persistence; sensor service discovery/start/stop;
BLE cadence through lid/charger/sleep changes; and routing opcodes `0x2B`, `0x44`,
`0x52`. Unknown traffic should stay labeled unknown until a controlled action and
repeatable payload change establish its meaning. Follow the
[capture procedure](MACOS-PROTOCOL-CAPTURE.md).

## Regression entry points

Tests live beside the implementation unless linked separately. Useful names:

- `aap.rs`: `device_controls_require_complete_valid_records_and_keep_both_value_bytes`,
  `native_eq_matches_librepods_and_rejects_truncated_or_unknown_forms`,
  `malformed_ear_reports_cannot_pause_playback_or_restart_the_microphone`.
- `device_settings.rs`: `replies_during_a_send_are_not_replaced_by_pending_state`,
  `silent_strength_requests_do_not_replace_reports_or_time_out`,
  `pro3_allow_off_writes_stay_requested_without_a_confirmation_timeout`.
- `battery_ble.rs`: `aes_matches_nist_vector_and_rpa_matches_independent_capod_fixture`,
  `encrypted_report_requires_owned_address_and_preserves_exact_levels_and_unknowns`,
  `short_reconnect_does_not_discard_a_fresh_ble_report`,
  `buds_only_reports_do_not_refresh_an_old_case_or_its_charging_flag`.
- `aap/accessory.rs`: `role_preserves_unassigned_side_and_flags_without_changing_policy`,
  `opaque_case_and_setup_reports_preserve_bytes_but_do_not_log_them`.
- `nearby_finder.rs`: `stale_owner_cannot_stop_new_client_or_new_pair`,
  `unavailable_identity_never_activates_scan`,
  `median_ema_trend_and_age_use_only_valid_monotonic_samples`.
- `head_tracking.rs`: `signed_axes_decode_across_every_fragment_boundary_and_batched_frames`,
  `only_live_exact_motion_records_are_accepted`,
  `old_client_disconnect_cannot_stop_new_owner_and_session_end_never_rearms`.
- `hearing.rs`: `stored_mode_packets_preserve_the_other_mode_enrollment_and_every_curve_byte`,
  `refreshed_mode_verification_checks_every_profile_byte_and_never_writes`,
  `mode_confirmation_rejects_changed_enrollment_even_when_off_flags_match`,
  `loud_sound_writes_only_the_characteristic_and_confirms_readback`,
  `loud_sound_rejects_missing_malformed_or_unacknowledged_replies`.
- [`host_clock.rs`](../windows/daemon/src/host_clock.rs),
  [`capabilities.rs`](../windows/daemon/src/capabilities.rs) and
  [`hearing_diagnostics.rs`](../windows/daemon/src/hearing_diagnostics.rs)
  keep their wire-format checks beside their parsers.
- [DeviceSettings tests](../windows/tests/DeviceSettings) check profile gating and
  presentation. [Windows test instructions](../windows/tests/README.md) describe
  the backend and native checks.
