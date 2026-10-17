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
| `0x0009` | Both | One-byte setting ID, then a four-byte little-endian value; exactly 11 bytes for `parse_device_control` |
| `0x000F` | H → B | Four-byte notification mask; the normal writer sends all bits set |
| `0x0017` | Both | RTBuddy/SensorDataWX framing; see sensor section |
| `0x001A` | H → B | Rename: `01 length 00`, then UTF-8 bytes (`build_rename`); command handler allows 1–64 bytes and rejects blank names |
| `0x001D` | B → H | Device metadata; strings begin at offset 11 (`parse_metadata`) |
| `0x002E` | B → H | Connected-host list; count at offset 8, entries begin at 9 |
| `0x0030` | H → B | BLE key request payload `05 00` |
| `0x0031` | B → H | BLE key response; never forwarded to UI or general packet logs |
| `0x004B` | B → H | Conversation Awareness event; status at offset 9 |
| `0x004D` | H → B | Feature mask from session setup |
| `0x0058` | Both | High-quality microphone control/audio; [audio reference](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md) |
| `0x0063` | Both | Three-band EQ: `05 00 01 mode low mid high`; exactly 13 bytes |

[`capabilities.rs`](../windows/daemon/src/capabilities.rs) keeps reported capability
IDs and raw values in the IPC `aacp_capabilities` map for the current control
connection. IDs `03`–`08` and `30` carry uint32 LE values; other recognized IDs
carry one byte. Entries have no length field, so unknown widths, duplicate IDs,
truncated values and trailing bytes are rejected without replacing the previous
report. Reconnecting clears the map. Values use different schemes across IDs;
this map does not change feature availability or activate hearing modes.

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
| `0x0A` | Ear detection | `1` enabled, `2` disabled |
| `0x0D` | Listening mode | `1` Off, `2` ANC, `3` Transparency, `4` Adaptive |
| `0x16` | Press-and-hold action | `v0` right, `v1` left; each `1` noise control or `5` assistant; `v2=v3=0` |
| `0x17` | Press speed | `0` Default, `1` Slower, `2` Slowest |
| `0x18` | Hold duration | `0` Default, `1` Slower, `2` Slowest (current UI labels) |
| `0x1A` | Stem listening-mode cycle | Mask: Off `1`, ANC `2`, Transparency `4`, Adaptive `8`; at least two bits set |
| `0x1B` | ANC with one bud | `1` enabled, `2` disabled |
| `0x1C` | Max crown direction | `1` reversed, `2` default |
| `0x1F` | Tone volume | `v0=0..100`, `v1=0x50`, `v2=v3=0` |
| `0x20` | Connect automatically | `1` enabled, `2` disabled; device preference, not the Windows reconnect switch |
| `0x23` | Volume-swipe duration | `1` Default, `2` Longer, `3` Longest |
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

Model allowlists and firmware checks in
[device_models.rs](../windows/daemon/src/device_models.rs) apply in addition to
value validation. A setting normally needs a valid device report before NTPods
allows a write. The Pro 3 Allow Off exception permits a silent write and saves
the choice per paired identity. Unknown models get only a conservative subset
of reported controls; they do not inherit Pro 3 EQ, microphone or sensor support.
Adaptive requires public firmware major 6+, sleep detection 8+, and head tracking
a recognized public firmware version. Profiles with an Allow Off option require
a report, a saved Pro 3 request, an already reported Off mode, or legacy firmware
major 1–6. Other listening profiles permit Off unless the device reports it disabled.

`PendingSettings` keeps reports separate from requests. Most writes wait up to ten
seconds for a matching report, starting after the transport write completes.
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
contain left-primary reports, but do not validate every role flag. NTPods does
not currently parse this message; its pause and microphone policies work on
both wear slots without assigning physical sides.

Conversation event `0x4B` is separate from the enabled setting `0x09/0x28`.
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

Scanning while connected is opt-in; capture or an active microphone uplink always
stops scanning. Verified battery advertisements keep a closed case's battery
visible without reconnecting audio. Automatic connection requires the report to
indicate availability outside the closed case, with per-visit suppression;
Max battery reports never trigger it. No advertisement can prove whether the
case is currently charging if its case slot is absent.

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
`devmotion6` and `HeartRate` dictionary/layout pairs to discover report service
IDs; heart-rate activation still uses the separately tested IDs 84/19.

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

## ATT settings

[hearing.rs](../windows/daemon/src/hearing.rs) and
[transparency.rs](../windows/daemon/src/transparency.rs) share
[UUID discovery](../windows/daemon/src/att_settings.rs) for hearing settings and
Custom Transparency. Loud Sound Reduction still uses its fixed upstream handle;
that assumption needs comparison on each model/firmware.

| Handle | Operation | Current format |
| --- | --- | --- |
| `0x001B` | Loud Sound Reduction | Read response `0B value`, value `0`/`1`; write `12 1B 00 value`, require `13`, then read back |
| Discovered (`0x002A` in captures) | Hearing-assistance settings | Read 104 value bytes; validate the format, both audiograms and per-ear controls; preserve the header; use the advertised write method and confirm by readback |
| `0x002B` in captures | Settings CCCD | macOS subscribes here; NTPods does not change hearing subscriptions |

Separate custom Transparency settings were identified in the
[macOS captures](MACOS-CASE-CAPTURE-2026-10-12.md#custom-transparency-and-accommodation)
at characteristic UUID `D5621CC1-F7AB-45DB-9403-9EAF744D5390`, handle `0x0018`
on the tested pair. [transparency.rs](../windows/daemon/src/transparency.rs)
discovers service `7798082B-B7B7-45A6-9933-563492EFE04E` and this characteristic by
UUID. Its experimental editor accepts exactly 104 bytes, reads before changing
them, preserves both eight-band curves and own-voice bytes, and reads back after
writing. The captured characteristic advertises Read, Write Without
Response and Notify (`0x16`), so NTPods sends a Write Command (`0x52`) without
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
hearing values have identical zero per-ear curves; hardware hearing-write
semantics remain unverified. The pairing trace establishes:

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
connection. NTPods accepts both exact 104-byte layouts for read-only display,
with the same scalar and audiogram validation. It blocks fitting edits in those
formats; the separate mode controls can activate an already enrolled profile
without rewriting it. Other unknown headers
remain unsupported. The existing formats with byte 2 equal to `0` or `0x64`
remain editable when the characteristic advertises writes. Their original
header is preserved; the writer does not force `0x64`. No hearing-value write
was captured, so header semantics and hardware write behavior remain unresolved.

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
| 0–3 | Preserved header; captured `02 00 60 00` and `02 02 60 00` are read-only |
| 4 / 52 | Left/right eight-band audiogram, eight little-endian float32 values |
| 36 / 84 | Left/right amplification |
| 40 / 88 | Tone |
| 44 / 92 | Conversation Boost, float `0`/`1` |
| 48 / 96 | Ambient-noise reduction |
| 100 | Own voice |

Mode-only enables read the enrolled fitting first and verify its complete bytes
afterward; mode-only requests never write `A4120005` or notification subscriptions.
Device reports, rather than successful sends, confirm both switches. NTPods
refreshes the control connection after a mode change while the microphone is
idle, then checks fresh reports. Enables also verify the unchanged fitting;
disables do not read it. Missing or inconsistent mode reports leave the result
unconfirmed. An explicit Off retry retains its last confirmed raw template
within the same connection; reconnects discard that template.

Hearing assistance has an explicit read operation that sends no enable command
or notification subscription. Fitting edits require that successful read
in the current connection, complete eight-band curves for both ears and a known
AAP enabled state. Before a fitting edit's enable command or settings write,
the backend compares the caller's original baseline with the latest confirmed state, reads
again and rejects any mismatch. A refresh by another client cannot authorize an
older draft. It retains the original value across the enable sequence,
preserves opaque bytes and requires matching settings and
header on readback. Discovery repeats after a new enable before choosing the
write handle and method.

The editor starts from those confirmed per-ear values. Scalar edits retain both
audiograms and untouched per-ear controls, including noise reduction and own
voice. Empty or incomplete curves are rejected. Reads accept amplification
from −1.5–1.5, matching Apple's version-2 reader. The editor still offers −1–1
amplification and tone, 0–90 dB HL thresholds, and 0–1 noise reduction and own
voice. A saved amplification outside the editor's range stays read-only.
Unsupported values are rejected rather
than clamped. Failed reads or writes require another read before editing; a
reconnect clears the saved baseline. The UI uses a session-bound editability
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

### Diagnostic and dosimetry records

These records belong to service `7798082B-B7B7-45A6-9933-563492EFE04E`.
NTPods has no diagnostic trigger or diagnostics UI. Its offline decoder reads
saved 71-byte records without connecting to the buds:

```powershell
cargo run --locked --manifest-path windows/daemon/Cargo.toml --example decode_hearing_diagnostics -- record.bin 2027
```

The optional final argument is the hexadecimal product ID. Without it, component
flags remain uninterpreted. The formats below come from captured traffic and pinned Apple-framework decompilations;
they are not a published protocol specification.

| Characteristic UUID | Value format |
| --- | --- |
| `A4120001-95C5-4D6F-9098-0F0B41457E0A` | One observed byte; meaning unresolved |
| `A4120002-95C5-4D6F-9098-0F0B41457E0A` | Hardware diagnostic status; observed version 8 is 71 bytes |
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
microphones. Their raw `0`/`1` labels are `Pass`/`Fail`, consumed only for
measurement status `0` or `1`, a nonzero measurement timestamp, and product IDs
`0x2014`, `0x2024` or Pro 3 `0x2027`. The same products pass the occlusion-support
gate, but an occlusion
assessment also needs accumulated fault history and indication state. ANC
scores have a separate gate: product `0x201B` with record version at least 4;
Pro 3 does not pass it. These cached fields alone are not a current health
verdict. [Later source changes](https://github.com/blacktop/ipsw-diffs/blob/af456047befbab058df4c3d7c7094462ad4753d6/iOS/26_5_23F77_vs_27_0_24A5355q/DYLIBS/System/Library/PrivateFrameworks/HearingModeService.framework/HearingModeService.md#L85-L158)
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
The subtype-2 arrays' frequency bins and physical units remain
unverified, and the record does not establish a 24-band fitting transform.

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
