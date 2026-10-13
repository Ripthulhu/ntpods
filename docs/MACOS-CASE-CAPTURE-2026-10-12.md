# Case and sensor captures, 2026-10-12

AirPods Pro 3 on 9A348, using the Tahoe Intel AX200 VM described in the
[capture guide](MACOS-PROTOCOL-CAPTURE.md). Two runs covered removing the buds,
returning them, closing the lid and connecting the charger. The second run
connected the charger while the lid was open, then closed it.

The observed battery, charging and lid fields agree with NTPods. Other
messages remain partially decoded or unused.

## Battery and case results

| Observation | Evidence | NTPods coverage |
| --- | --- | --- |
| Individual buds leave and return to the case | AAP battery/wear reports and identity-matched BLE reports | Existing component charging and wear handling |
| Case reading becomes unavailable while both buds are out | AAP reports both `0/status 4` and `255/status 4` | Unavailable status preserves the last known percentage |
| Case starts charging | First run: AAP case 91%, status changes from `2` to `1` | Existing AAP charging flag |
| Charger connected with lid open | Second run: BLE case 93%, charging bit changes to true | Existing BLE case charging flag |
| Lid closes while charging | Second run: BLE closed-lid flag changes to true | Existing BLE lid interpretation |

The BLE check resolved private addresses with the paired identity key before
filtering advertisement types. All matching Apple manufacturer reports were
type `0x07`; none of another type matched that identity in either run.

An isolated harness included production
[battery_ble.rs](../windows/daemon/src/battery_ble.rs) unchanged and ran
`parse_keys` and `Keys::decode` through Windows BCrypt. It compared results with
an independent interpretation of the captured fields:

| Run | Apple manufacturer reports checked | Reports accepted for the paired buds | Disagreements |
| --- | ---: | ---: | ---: |
| Bud removal, return and charger | 816 | 106 | 0 |
| Charger with lid open, then closed | 742 | 96 | 0 |

Keys and captured payloads passed through process memory and stdin, without
writing NTPods' key cache. This checked the decoder rather than the Windows
advertisement watcher or UI. AAP battery and wear fields were checked against
`parse_battery`, `parse_ear_detection` and their state handling in
[aap.rs](../windows/daemon/src/aap.rs).

## Closed-case reporting

In the first run, the last matching BLE report preceded the AAP case-charging
report by about 255 ms. The case therefore reported charging through the live
control connection even though no later matching BLE report arrived.

In the second run, BLE reported the case charging at 21:44:37.563 UTC and the
lid closed at 21:44:51.604. The last matching report arrived at 21:44:54.341;
none followed through 21:46:37. Scanning continued and received other devices.
The controller used active 30 ms scan windows every 300 ms, without duplicate
filtering. That 10% scan duty cycle cannot establish that the buds transmitted
nothing over the air.

These runs establish the charging and lid fields, but do not establish sustained
closed-case updates in this VM. Retain the distinction between a fresh report
and cached battery data.

## Messages beyond current coverage

Both runs contained the following traffic. Frame references below use A for
the first run and B for the second. Names from upstream decoders
remain reference-derived unless the captured fields independently establish them.

| Message | Current coverage or unresolved question |
| --- | --- |
| AAP `0x02`, capabilities | 41 entries, identical across both runs; names and multi-valued fields need individual validation |
| AAP `0x08`, bud role | First byte identifies the primary side in LibrePods; a later flag changes before secondary disconnection |
| AAP `0x23`, CaseInfo | Identical 25-byte response to `0x22`; tentative identity/version fields, no established battery or lid field |
| Control `0x09/0x38` | Reports 82; reference calls it PPE cap-level configuration, but units and writable range are unknown |
| AAP `0x4C`, Adaptive Volume | Same two setup payloads in both runs; field meanings remain unknown |
| AAP `0x4E`, proximity card | Eight zero bytes during setup in each run; no state change observed |
| AAP `0x53` | Matches the eight-band Headphone Accommodation layout; disabled flags and zero EQ in both runs |
| AAP `0x54` | Radio-band configuration; distinct from audio EQ |
| AAP `0x55` | Four unchanged bytes; meaning unknown |
| RTBuddy `0x17` | Device-authored service names and HID descriptors; no live sensor samples in these runs |
| UARP `0x4F` | Accessory information, metadata and small assets; no firmware apply command |
| ATT handle `0x0018` on this device | Custom Transparency configuration, matched by UUID discovery and LibrePods' layout |

### Capability, role and case messages

The capability bodies at A1199/B1405 contain 41 entries in 98 bytes. Values are
not uniformly boolean: ID `0x18` reports 3, for example. Reference labels include
heart rate (`0x26`), dynamic end of charge (`0x29`), hearing protection (`0x30`),
hearing aid V2 (`0x31`) and preference EQ (`0x37`), each reporting 1 here. A
capability report alone does not validate the feature or its control sequence.
NTPods currently uses its shared model catalog rather than parsing this message.

Role `0x08` has a four-byte body. LibrePods maps body byte 0 as 1=left primary,
2=right primary; only 1 occurs in the two case runs. The body changes from `01 00 01 00` to
`01 00 00 00` just before a secondary-disconnected wear report in both runs
(A4550/4559, B3821/3826). Byte 2 may describe the secondary link, but these
captures do not establish all its values. NTPods' current pause and microphone
policies treat the two wear slots symmetrically and do not need this role map.

Empty CaseInfo requests at A1201/B1407 receive identical 25-byte replies at
A1229/B1487, 213/158 ms later. The reference tentatively splits the body into
version, vendor/product fields and 16 opaque bytes. No observed change ties
these fields to the charger or lid. Do not use them as a battery report or a
replacement for the paired BLE identity key.

Adaptive Volume `0x4C` carries a little-endian length of 3 followed by either
`04 02 00` or `04 3A 01` (A1211/1215, B1426/1429). Both occur during setup,
before notification registration. They provide no demonstrated mapping to
Personalized Volume (`0x09/0x26`) or Adaptive strength (`0x09/0x2E`).

### Sensor descriptions

RTBuddy descriptor field 5 contains a service ID and an
[OSSerializeBinary dictionary](https://github.com/apple-oss-distributions/xnu/blob/main/libkern/libkern/OSSerializeBinary.h).
The names below come from the buds' dictionaries. The two runs agree:

| Service ID (decimal) | Device-supplied name | MaxReportSize | HID descriptor bytes | Frames A / B |
| --- | --- | ---: | ---: | --- |
| 14 | `activity` | 183 | 65 | 1251 / 1493 |
| 16 | `devmotion6` | 191 | 96 | 1242 / 1482 |
| 18 | `SPL0` | 57 | 33 | 1251 / 1493 |
| 19 | `HostLibHID` | 70 | 313 | 1258 / 1505 |
| 20 | `HeartRate` | 601 | 126 | 1258 / 1505 |

The pinned community decoder assigns SPL0/HostLibHID/HeartRate to 17/18/19;
those fixed labels are shifted on this firmware. NTPods already accepts heart
rate on services 19, 20 and 84, and uses motion service 16 on firmware 8+.

The HeartRate HID descriptor independently confirms report ID 1 as 17 data
bytes plus the ID, matching NTPods' 18-byte record. Its first data field is
standard Sensors usage `0x04B8`, heartbeats per minute, at record byte 1.
The confidence and status fields used by NTPods remain vendor-defined in HID.
Report ID 2 is 601 bytes including its ID, explaining the larger MaxReportSize;
these captures contain no live report of either kind.

SPL0 declares a 57-byte input report and a 13-byte feature report. Both contain
vendor-defined fields without a decibel unit or scale. Its name suggests sound
level, but the descriptor alone cannot support a sound-level display.

The HeartRate dictionary also names `com.apple.hid.heartrate-access` as a
macOS HID entitlement. That metadata does not establish an over-the-air
authorization requirement for NTPods. Serial and configuration identifiers in
the dictionaries are omitted from this report.

### Custom Transparency and accommodation

Pairing discovery identifies service `7798082B-B7B7-45A6-9933-563492EFE04E` and
characteristic `D5621CC1-F7AB-45DB-9403-9EAF744D5390`, at value handle `0x0018`
on this pair. Its properties allow Read, Write Without Response and Notify.
The pairing capture establishes the UUID at frames 5001/5106 and the CCCD
at 5362. Fixed handles must not be assumed on another model or firmware.

Across pairing and both case runs, all 118 read responses and 37 Write Commands
carry the same 104 bytes: 25 float32 little-endian zeros followed by 0.5.
This matches LibrePods' custom Transparency layout:

| Value offset | Reference-derived field |
| --- | --- |
| 0 | Enabled, float32 |
| 4–35 / 52–83 | Left/right eight-band EQ, float32 |
| 36 / 84 | Amplification |
| 40 / 88 | Tone |
| 44 / 92 | Conversation Boost |
| 48 / 96 | Ambient-noise reduction |
| 100 | Optional own-voice amplification |

Only this unchanged value was observed; individual field effects need
controlled testing. The similar hearing-assistance layout at `0x002A` has a
different header, so its encoder cannot be reused unchanged. NTPods did not
implement custom Transparency settings at capture time. The later
[experimental implementation](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#att-settings)
uses UUID discovery, bounds and finite-float checks, preservation of unknown
bytes and readback after writes. Its Windows read failure and acoustic testing
remain unresolved.

AAP `0x53` carries a two-byte length followed by 132 bytes: a four-byte prefix
and four blocks of eight float32 values. These match LibrePods' Headphone
Accommodation format. Both captures have zero EQ and both phone/media flags
disabled. Upstream's sender and receiver reverse the phone/media labels at
packet offsets 10/11; equal captured values cannot resolve that discrepancy.
This is separate from NTPods' implemented three-band EQ at `0x63`.

Radio configuration `0x54` begins with a one-byte count, then seven three-byte
records: band index, low edge, high edge. The reference names ISM 2.4 GHz and
UNII bands; every edge byte here is 255. Their units and sentinel meanings
remain unknown. `0x55` contains `01 01 00 19` in both runs, with no verified
interpretation.

### UARP information and assets

AAP `0x4F` carries a little-endian uint16 length followed by UARP. Its header
uses big-endian uint16 message type, body length and sequence. Type `0x1B`
wraps a big-endian uint16 downstream endpoint and another UARP message. Both
runs negotiate version 6 after macOS requests 7, at the root and downstream
endpoints 1/2. Those endpoint IDs have not been assigned to physical buds or case.

Each run has 21 information requests and 21 successful responses across those
three endpoints. Reference labels include manufacturer, model, serial and
current/staged firmware. Identity values remain private.

| Asset | Direction | Observed structure in each run |
| --- | --- | --- |
| `MTIC` | B → H | 18 assets in A, 10 in B; small SuperBinary assets, 206–1124 bytes |
| `HSML` | B → H | 2171 bytes, format 2, 23 metadata-only rows |
| `VERS` | B → H | 132 bytes, format 2, top-level metadata, no rows |
| `HSML` | H → B | 145 bytes, format 3, one metadata-only row |

All assets reassemble with complete offset coverage and valid bounded
header/row/TLV fields. The UARP template labels HSML metadata types
`0x608B0E00/01/02/03/07/08` as Hey Siri model type, locale, hash, role, engine
version and engine type. These source-derived labels support a Siri model
inventory and selection exchange; their behavior was not tested here.
MTIC's field meanings need a matching event schema, and VERS semantics remain
incomplete. Neither run contains an Apply Staged Assets request or response,
including nested messages. These exchanges do not establish a firmware
installation or another battery path after the control connection closes.

### Wearing and movement follow-up

The wearing and movement capture ran from 22:12:48 to 22:14:36 UTC. It began with
the buds in the case, then covered putting both in the ears and marked head
turns/nods. User confirmations follow the physical actions, so their timestamps
are not exact insertion times.

The link connected once at frame 317, with no HCI disconnect event afterward.
Role `0x08` changed from `01 00 01 00` to `02 00 01 01` at frame 2144
(22:13:17.839 UTC). The reference mapping identifies a left-to-right primary
handoff. A second set of sensor announcements followed at frames 2174–2210;
their sequence restarted and log type changed from 3 to 1. These changes
therefore do not require a new Bluetooth connection.

Battery component records reordered at frame 2177 while retaining right 95%
and left 93%. Parsers must use component IDs rather than record position.
Adaptive Volume repeated its same two setup payloads around the handoff.

All five HID report descriptors matched the case captures byte for byte.
Some ancillary properties changed, including LocationID; no report layout
changed. Across all 13 RTBuddy packets there were no field-7 reports, field-8
stream-setting requests or field-3 opaque streams. No RTBuddy packet occurred
during the marked head movements. Log type 1 alone cannot identify a live
sample: it also occurred on these descriptor announcements.

Passive wearing and movement produced no sensor measurements in this run.
A sensor-specific comparison needs a verified request to start the service.
The Transparency characteristic remained unchanged across 21 reads and eight
writes; each value matched the earlier 104-byte configuration. No new ATT
discovery changed the UUID mapping.

No new AAP opcode family or UARP asset tag appeared. Completed HSML/VERS assets
matched the case runs byte for byte. UARP also repeated discovery and added
downstream acknowledgments. Four transfers were incomplete; three later assets
contained only their headers, so their missing contents cannot be called
unchanged. Reused asset IDs require separating transfers by their announcement.
Fourteen MTIC assets completed, with some new lengths but no established event
field meanings. No firmware apply command appeared. The capture contains
13,425 frames with no drops, truncation or Lua errors. Wireshark's native
dissector marks one Read Clock command-complete event at frame 1037 malformed;
the frame is fully captured. That unresolved warning concerns HCI clock
decoding, outside the AAP fields analyzed here.

### Active SPL0 probes

Native IOHID access on Tahoe reaches SPL0 through macOS's existing AAP
connection. A 12-second open/close alone produced no RTBuddy traffic.
`IOHIDDeviceGetReport` with feature type 2 and report ID 1 returned five bytes,
`01 00 00 00 00`, despite the descriptor's 13-byte feature capacity.
The remaining eight descriptor bytes are still unexplained.

The read maps to SensorDataWX field 10: nested field 1 is service 18, field 2
is report type 2, and field 3 is report ID 1. The reply uses field 11, with
service 18 in nested field 1 and report bytes in nested field 3. A feature
write maps to field 8 with service 18, type 2 and the complete report bytes;
field 9 acknowledges it.

The final SPL0 capture tested `01 40 42 0F 00`, using the 1,000,000 µs
interval encoding known from the other sensors. Both buds were in the ears.
Readback confirmed that exact value, and a native input listener then ran for
12 seconds. It received zero callbacks. The wire capture also contains no
field-7 reports, field-3 streams or alternate AAP payloads. The setting was
nonzero for 13.20 seconds between writes. Its units and effect on SPL0 remain
unverified because no measurements arrived.

Restoring `01 00 00 00 00` succeeded in both native and wire readback. The
capture continued for 11.39 seconds after the stop write, with no samples.
A separate Input report 1 read with a 57-byte buffer received no wire reply
and timed out. This differs from the earlier feature-read timeouts, whose
replies did arrive over Bluetooth.

During the first probes, SetReport returned before the device acknowledgment,
and macOS sometimes retried after 200 ms. GetReport failures coincided with
outstanding write acknowledgments, even when the buds answered the read.
Waiting one second between a write and readback worked in the final run;
the cause of the earlier local failures remains unverified. Acknowledgments
and retained configuration alone do not establish a working stream.

The final capture has 388 frames, with no drops, truncation, Lua errors,
malformed markers or HCI connection/disconnection events. All 15 AAP messages
are SPL0 RTBuddy transactions. Raw captures and the bounded probe helpers stay
outside Git. No SPL0 decoder or sound-level UI was added.

### Native head-tracking comparison

A temporary CoreMotion app requested headphone motion through macOS. Direct
launch over SSH left authorization undetermined and produced no motion reports.
Launching through LaunchServices displayed the Motion prompt; after approval,
two captures received live service-16
reports. The JXA callback counter stayed at zero, so these results use the
Bluetooth capture rather than the app's counter.

Both runs use field-8 feature writes with report type 2:

| Action | Report bytes | Descriptor-derived meaning |
| --- | --- | --- |
| Start, first write | `04 01 00 00 00` | Feature 4, Maximum FIFO Events = 1 |
| Start, second write | `01 20 4E 00 00` | Feature 1, Report Interval = 20,000 |
| Stop, first write | `04 0A 00 00 00` | Feature 4, Maximum FIFO Events = 10 |
| Stop, second write | `01 00 00 00 00` | Feature 1, Report Interval = 0 |

The devmotion6 descriptor assigns feature 4 to Sensors usage `0x20:031A`.
The HID Usage Tables label it Maximum FIFO Events; the effect of Apple's
writes on batching has not been measured. SPL0 does not declare feature 4.

The first run has 599 reports across 11.99 seconds, about 50 per second.
Each uses top-level field 3 with 58 bytes: `10 00` followed by 56 bytes of
report data beginning `01`. Log type 5 packets also have a two-byte `FF FF`
trailer after the protobuf, included in the outer RTBuddy length. Parsing
that trailer as protobuf causes false truncation errors.

The second run has 581 field-3 reports and one field-7 service-16 report with
56 data bytes. One field-3 report shares a log-type-1 envelope with an
acknowledgment and has no trailer. The field-7 report arrives 5.10 ms after
the zero-interval write, before its acknowledgment; nothing follows the
acknowledgment during the remaining 3.38 seconds. The first run has no
reports after its zero-interval write and a 3.41-second capture tail.

At capture time, NTPods accepted field 7 with 58 data bytes and log types 1/3.
The later [sensor implementation](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#sensors)
also accepts these macOS envelopes. The two-byte service prefix must not
shift sensor offsets when comparing the 58-byte packed form with the
56-byte report data. Their field units and pose interpretation remain
unverified. Both probes stopped and exited; Bluetooth stayed with Tahoe.
Neither capture has drops, truncation or RTBuddy parse errors. Each has 11
native Wireshark Read Clock warnings on fully captured HCI frames; those
warnings remain unresolved and do not concern the sensor payloads.

### Sources

- [Apple Wireshark dissectors, pinned revision](https://github.com/pabloaul/apple-wireshark/tree/2e9822247195fad2f79c729010308c33a29546ea), especially AACP enums and RTBuddy/UARP parsers.
- [LibrePods Transparency layout](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/att/types/Transparency.kt) and [ATT mapping](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/att/ATTManager.kt).
- [LibrePods AACP parser and accommodation writer](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/aacp/AACPManager.kt).
- [USB HID Usage Tables](https://usb.org/sites/default/files/hut1_6.pdf), Sensors page; report framing follows the [HID 1.11 specification](https://www.usb.org/sites/default/files/hid1_11.pdf).
- [UARP SuperBinary template](https://github.com/dhinakg/templates/blob/main/uarp.tcl), compared with the reconstructed asset bounds.

## Capture quality and limits

The first and second case runs contain 9,146 and
8,808 frames, with no capture drops or truncated frames. Fresh FastConnect setup
provides channel mappings. Three local guards fix valid empty-tail reads in the
pinned Lua dissectors; twelve synthetic tests pass, including malformed-length
checks. Both traces then decode without Lua errors. Raw captures and the pinned
upstream checkout are unchanged. Separate bounded parsers checked capabilities,
RTBuddy dictionaries, HID report sizes and complete UARP assets in both runs.
The capability, protobuf and dictionary parsers also passed synthetic truncation
checks. Only sanitized summaries were saved.

The captures show traffic between this host and the buds. They do not expose
all bud-to-bud or bud-to-case radio traffic, prove every field's meaning, or
establish coverage for other AirPods models. Raw traces and unredacted logs stay
outside Git.
