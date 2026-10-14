# AirPods control, battery and sensor protocol

This reference describes NTPods' reverse-engineered Windows implementation.
Packet offsets are zero-based; hex values
have a `0x` prefix. `H → B` means host to buds and `B → H` means buds to host.

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
| H → B | `04 00 04 00 30 00 05 00` | Request keys for paired-device BLE battery reports |

The `0x4D` mask enables features used by Adaptive and Conversation Awareness on
tested devices. The meaning of every mask bit is unknown. The delays are current
host behavior, not measured protocol requirements. NTPods marks its control
connection up after sending this sequence; individual reports establish settings.

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

The receive path recognizes `0x2B`, `0x44`, and `0x52` for bounded route diagnostics
but does not decode their state. Their names in `route_opcode_name` are working
labels inherited from research. A `0x2E` list contains six-byte big-endian host
addresses followed by two flag bytes. The flags are not a reliable device type:
the same PC has reported several values. The parser retains complete entries
from a truncated list, so such a list is not evidence that omitted hosts left.
The daemon's existing `route:` log lines include up to 64 packet bytes, including
addresses in `0x2E`. Redact identities in those logs as well as in raw captures.

Metadata parsing requires eleven NUL-terminated UTF-8 strings after the five-byte
information prefix. NTPods uses string 1 for model and string 3 for serial
(indices start at zero). For firmware it prefers a public version in string 10,
then string 4, then a recognized packed release value in string 10. The packed
conversion is inferred: major `value >> 20`, letter `A + ((value >> 16) & 15)`,
build `(value >> 4) & 4095`, with a zero low nibble. Unknown/internal versions do
not grant firmware-dependent capabilities. Trailing identity data is unparsed.
Do not publish names, serials or host addresses from a capture.

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
Adaptive strength requires a confirmed Adaptive listening mode. Pro 3/9A348
reconnect reports were coarse (0, 50, 100); that observation did not establish
three audible levels. The current app sends the full 0–100 range and listening checks
confirmed an audible effect. A macOS comparison should retain both the requested
value and each later report.

EQ mode `1` is recommended sound; mode `2` selects custom low/mid/high values,
each `0..100`. The implementation requires a matching EQ-capable profile and
public firmware major at least 9. This is opcode `0x63`; the older `0x53`
headphone-accommodation format is not the app's EQ writer.

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
parses descriptor announcements with bounded field counts and dictionary lengths. Parse nested fields rather than fixed offsets
into an entire Bluetooth packet; sequence varints and wrappers change its size.

Head tracking uses [head_tracking.rs](../windows/daemon/src/head_tracking.rs).
A recognized device descriptor selects the native path described below. Without
one, the firmware fallback uses this format. The outgoing payload is field 1 sequence, field 8 ServiceSettings containing
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
uses service 16, first writing feature report `04 01 00 00 00` (Maximum FIFO
Events = 1), then `01 20 4E 00 00` (interval 20,000). Stop writes FIFO value
10 followed by interval zero. Captured reports arrive at about 50 Hz in a
different envelope: top-level field 3, a two-byte service prefix and 56 bytes
of report data. Log type 5 also carries a two-byte trailer after the protobuf.
A final field-7 report contains 56 bytes. NTPods accepts both native forms after
matching the announced `devmotion6` HID layout, strips the service prefix and
log-5 trailer, then reads report-relative raw channels at offsets 28 and 30.
Windows Pro 3 firmware 9A348 also sends a 60-byte packed field: the two-byte
service prefix followed by a 58-byte report. The A3063 primary used log type 1;
the A3064 primary used type 3. Neither Windows form had the log-5 trailer.
Those channels remain uncalibrated. It uses the captured 50 Hz/FIFO sequence only
for that descriptor; other firmware keeps the existing 25 Hz commands. The FIFO
setting's effect on latency remains unmeasured.

Heart rate uses [hr.rs](../windows/daemon/src/hr.rs), `aap::hr_stream`, and
`hr_retry_campaign` in [main.rs](../windows/daemon/src/main.rs). The runtime sends
the AACP 1.3 CONNECT/CAPABILITIES sequence for services 0 and 4, enables control
`0x30=1`, then requests 1,000,000 µs intervals on services `84` and `19`. Unlike the
head-tracking writer, `hr_stream` includes top-level field 2=`2` and a seven-bit
sequence. Disabling requests interval zero on both services. Existing descriptor
and raw PPG packet builders remain research helpers. Normal startup requests
sensor descriptors once per connection.

The HR decoder accepts live log types `1`/`3`, report services `19`/`20`/`84`, and
bounded nested command envelopes. An accepted record is exactly 18 bytes: byte 1
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
scale. Neither sensor descriptor is itself a live sample. NTPods recognizes the captured
`devmotion6` and `HeartRate` dictionary/layout pairs and discovers their report
service IDs. Heart-rate activation still uses the separately tested IDs 84/19.
A restarted descriptor sequence or mapping change clears partial sensor data and
queues recovery of user-enabled streams on the existing channel. Disabling a
sensor cancels that intent; discovery cannot re-enable it.
When a discovered motion address changes, NTPods retires the old address without
writing to it. A withdrawn mapping waits for a new descriptor; firmware fallback
remains available when no native mapping has been discovered.
The receiver only queues refreshes; the sensor worker takes the control locks
and sends the commands. New mappings replace queued older mappings. Heart-rate
refreshes retain the user's request generation and cannot restart an active
enable campaign in response to that campaign's own descriptor announcements.

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
Pro 3 reads, writes, an audible adjustment and restoration passed with the Apple
host identity. Persistence across reconnects remains unverified; see
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

The saved Pro 3 / 9A348 captures from Tahoe 26.7.1 (25G241) on the AX200 contain
successful 104-byte reads of both Custom Transparency and hearing settings. The
four hearing values are identical, with zero per-ear curves; this does not
verify reading or retaining a fitted audiogram. In the pairing
trace, encryption completed before ATT opened on BR/EDR PSM `0x001F`. Tahoe
subscribed to each characteristic before reading it. Both reads were requested
before AACP Connect; the hearing response also arrived before that exchange.
Both reads succeeded before the buds' `0x2C`/`0x33` state reports, and the host
sent neither hearing-enable control in that trace. This establishes that
the Intel adapter can carry these reads. It does not establish hearing-write
semantics.

Hearing settings use characteristic UUID `A4120005-95C5-4D6F-9098-0F0B41457E0A`.
The captured properties are `0x16` (Read, Write Without Response, Notify).
NTPods discovers the value handle and uses Write Command (`0x52`) when only
Write Without Response is advertised. If Write is advertised, it uses Write
Request (`0x12`) and requires the exact `0x13` response. A characteristic that
advertises only Read remains readable. Each write requires matching readback;
there is no notification consumer or fixed CCCD write.

Four successful hearing reads, across connection, case and wearing captures,
return header `02 00 60 00`. NTPods accepts this exact 104-byte layout for
read-only display, with the same scalar and audiogram validation. It blocks
enable and edits before sending any hearing controls. Other unknown headers
remain unsupported. The existing formats with byte 2 equal to `0` or `0x64`
remain editable when the characteristic advertises writes. Their original
header is preserved; the writer does not force `0x64`. No hearing-value write
was captured, so header semantics and hardware write behavior remain unresolved.

Windows 11 with AX200 and Pro 3 A3064 / 9A348 returned ATT `0x0E` for both
values without the Apple host identity. Reads succeeded after applying it and
restarting Windows, without re-pairing. Custom Transparency enable and per-ear
noise reduction of `0.125` produced an audible change and matching readback;
restoration preserved the separate hearing settings. NTPods 0.2.26 independently
read both restored values. The Windows SDP response has not yet been captured.

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

Hearing-assistance enable sends controls `0x2C=[01,01,00,00]`, listening mode 3,
then `0x33=1`, with delays, before opening ATT. Disable sends `0x33=2` then
`0x2C=[01,02,00,00]`. These are dedicated operations outside the generic setting
allowlist. Settings offsets relative to the characteristic value are:

| Offset | Field |
| --- | --- |
| 0–3 | Preserved header; captured `02 00 60 00` is read-only |
| 4 / 52 | Left/right eight-band audiogram, eight little-endian float32 values |
| 36 / 84 | Left/right amplification |
| 40 / 88 | Tone |
| 44 / 92 | Conversation Boost, float `0`/`1` |
| 48 / 96 | Ambient-noise reduction |
| 100 | Own voice |

Hearing assistance has an explicit read operation that sends no enable command
or notification subscription. Enabling or editing requires that successful read
in the current connection, complete eight-band curves for both ears and a known
AAP enabled state. Before any enable command or settings write, the backend
compares the caller's original baseline with the latest confirmed state, reads
again and rejects any mismatch. A refresh by another client cannot authorize an
older draft. It retains the original value across the enable sequence,
preserves opaque bytes and requires matching settings and
header on readback. Discovery repeats after a new enable before choosing the
write handle and method.

The editor starts from those confirmed per-ear values. Scalar edits retain both
audiograms and untouched per-ear controls, including noise reduction and own
voice. Empty or incomplete curves are rejected. The supported editor range is
0–90 dB HL for each threshold, −1–1 for each ear's amplification and tone, and
0–1 for noise reduction and own voice. Unsupported values are rejected rather
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
Custom Transparency enable requires a confirmed inactive hearing-assistance
`0x2C` report on hearing-capable models. The complete `02 00 00 00` payload is an
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
- `hearing.rs`: `only_the_write_acknowledgment_confirms_hearing_settings`,
  `loud_sound_writes_only_the_characteristic_and_confirms_readback`,
  `loud_sound_rejects_missing_malformed_or_unacknowledged_replies`.
- [DeviceSettings tests](../windows/tests/DeviceSettings) check profile gating and
  presentation. [Windows test instructions](../windows/tests/README.md) describe
  the backend and native checks.
