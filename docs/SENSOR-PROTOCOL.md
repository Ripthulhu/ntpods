# Sensor protocol contract v1

NTPods and LibrePods Android share this contract for the opt-in native AACP sensor
path. Both repositories carry identical `sensor-wire-v1.json` fixtures and this
document; update both copies together. Each implementation tests its local copy,
without requiring another checkout or a network connection. Audio, volume,
handover, microphone transport and dongle HID are separate contracts.

## Supported transports

| Transport | RTBuddy descriptor | Required motion declaration | Default wire interval |
| --- | --- | --- | --- |
| AirPods 3 RemoteHID | `0x00000800` | A2564/A2565, `devmotion6`, Apple vendor 1452, MaxReportSize 182, exact 96-byte HID descriptor | 40,000 us (25 Hz) |
| Native SensorDataWX | `0x00100000` | Existing IOCFSerialize dictionary: `devmotion6`, vendor 1452, MaxReportSize 191, same exact HID descriptor | 20,000 us (50 Hz); existing FIFO sequence |
| Recognized legacy WX | `0x00100000` | Existing public-firmware fallback, before native discovery | 40,000 us (25 Hz) |

Ordinary AirPods 3 is PID `0x2013`; Pro 3 is `0x2027`. RemoteHID is gated to
A2564/A2565, then to the exact descriptor; a firmware name alone never selects it.
Other models retain their existing capability gates. A service ID is local to a
connection, never hardcoded. The public AP3 capture happens to advertise motion
ID 7, gyro ID 5 and accelerometer ID 4. Only motion is subscribed by this change;
raw gyro/accelerometer services are research leads, not automatic extra streams.

Motion HID descriptor SHA-256:
`732e13269fbb399ff87ce3f183411b3fc03b9e7d6196dd9a99e3e77d51eb081c`.

## Framing and validation

Both transports begin `04 00 04 00 17 00`, descriptor u32 LE, body length u16 LE,
then exactly that body. Reassembly handles split and adjacent frames. Input chunks
are at most 16 KiB, carry plus input at most 32 KiB, and bodies at most 16 KiB
(WX) or 4096 bytes (RemoteHID). Unknown descriptor envelopes are skipped whole;
bytes inside another client's payload must not become sensor frames.

RemoteHID body starts with four bytes: version 0, generation, opaque byte and
side flags 0 or 2. The opaque byte is not necessarily zero. At most 32 subpackets
follow, each starting with a u32 LE header:

| Bits | Meaning |
| --- | --- |
| 0..6 | Device ID; 0 reserved for host discovery |
| 7..16 | Total subpacket length including header; 4..1023 |
| 17..19 | Kind: 0 Connect, 1 Disconnect, 2 Input, 3 SetReport, 4 GetReport, 5 ConfirmConnect |
| 20 | Optional Input timestamp |
| 21 | Response |
| 22 | Status |
| 23..31 | Must be zero for the supported grammar |

Validate the whole batch before changing mappings. Ignore response/status packets
for discovery and live samples. Ordinary Input contains type byte 0, then report
ID and data. Timestamped Input contains type 0, u64 LE timestamp, then report ID
and data. The timestamp follows the type byte, not the report ID. Supported times
fit signed 64-bit nanoseconds; zero means unknown. RemoteHID motion is exactly
58 bytes including report ID 1.

Connect XML uses a bounded plist subset: UTF-8, at most 1019 bytes, one plist
version 1.0 root with a flat dictionary, at most 24 unique keys of 128 UTF-8 bytes,
and at most 96 values with array nesting through level 4. Values are string,
integer, data, bool or array. One optional XML preamble (at most 100 bytes before
its terminator) and the exact captured Apple public DOCTYPE are inert text.
Entities, external DTD loading, comments, nested dictionaries, duplicate keys and
trailing roots are rejected. ReportDescriptor is canonical base64 with at most
512 encoded characters. Match service, vendor, maximum and the entire HID layout.

WX retains its established protobuf envelopes, log-5 trailer and exact native
HID checks. Opcode 0x17 is not universally WX protobuf. Existing legacy 58-byte
records can still feed the historical two-channel interface; full motion fields
require supported report ID 1 layouts.

## Shared motion model

WX 56/58-byte and RemoteHID 58-byte records normalize to the same field names.
Vectors contain exactly three signed 16-bit counts in device order. No implicit
cross-model conversion is applied.

| Field | Record offsets | Meaning |
| --- | --- | --- |
| `timestamp_ns` | 1..8 | Optional u64 LE sensor time; zero unknown; signed 64-bit maximum |
| `attitude_q15` | 20..25 | Three signed Q15 attitude components |
| `gyro_raw` | 26..31 | Raw angular-rate channels |
| `user_acceleration_raw` | 38..43 | Raw user-acceleration channels |
| `gravity_raw` | 44..49 | Raw gravity channels |
| `counter` | 54..57 | Optional u32 LE; absent in the 56-byte form |

An unknown inner time can use a valid positive RemoteHID transport time. Never
replace a known inner time. Missing counter/time does not imply a fresh sample.

Windows horizontal/vertical events and Android's historical
`HeadTracking.acceleration` flow stay compatible: these are gyro channels 1 and 2,
not physical linear acceleration. Android adds an accurately named
`angularVelocity` alias and full `motion` flow; Windows adds optional `motion` to
its existing event. Events without it remain valid. Existing gesture thresholds
are unchanged.

AP3-only helpers reconstruct the positive-w quaternion from the Q15 vector;
identity is valid, squared norm above 1.01 is rejected, and small overshoot is
normalized. Gravity-referenced rates use the published AP3 scale 0.122 degrees/s
per count and require a measured nonparallel lateral axis. Published AP3
gravity/user-acceleration scale is approximately 1000 counts/g. These scales are
not established for Pro 3, an absolute spatial pose or medical measurements.
Helpers are not activated automatically and do not alter gesture calibration.

## Ownership and lifecycle

1. Connecting or receiving a descriptor does not enable motion.
2. An explicit stream request owns the captured socket/session. AP3 without a
   mapping sends HostConnect once and waits for the exact declaration. Never
   substitute WX service 14/16 on AP3. WX keeps its existing discovery.
3. RemoteHID Start uses SetReport type 2, feature 1, interval u32 LE in us. Stop
   uses the same current ID/feature with interval zero. WX keeps its FIFO and
   interval commands.
4. Only a valid live current-service sample establishes Active. A write, ACK,
   descriptor, cached sample or unrelated sensor does not.
5. Re-announcements rearm only an owned stream. Same-ID announcements can
   stop/start that current mapping. Changed/withdrawn IDs invalidate old samples
   and cleanup addresses; never send delayed Stop to a repurposed ID.
6. Stop/suspend/owner retirement stops or discards the owned stream. Failed Stop
   retains cleanup, retries once automatically and blocks new Start until cleanup
   succeeds, mapping invalidates or the owning connection ends. A stale socket
   cannot publish samples or write to its replacement.
7. Five seconds without live samples times out. Failed writes cannot create an
   unbounded Start loop. Disabled streams remain disabled after discovery.

Android's debug interval now updates the active owned stream, persisting only
after success. Requests are whole milliseconds from 20..1000 ms; legacy WX is
clamped to at least 40 ms. Opening the screen does not send the initial slider
value. Old preferences were never applied, so an explicit-choice marker preserves
current defaults. Windows keeps the fixed intervals above. No 200 Hz default is
introduced.

## MTU and dongle boundary

The public AP3 MTU-672 capture contains three announcements up to 649 bytes; the
MTU-2582 capture contains eight up to 892 bytes (motion frame 833). This does not
establish 2582 as a minimum. Windows receive MTU 1691 already fits every supplied
frame. The dongle declares AACP 4096, but its HCI buffers cap actual receive MTU at
1691, also enough for these frames. Android's negotiated MTU and first-success
Apple identity requirements have not been measured by this change. MTU, audio
buffers and identity policy are unchanged.

Current dongle HID does not expose this motion model. This patch adds no sensor
capability bit or firmware subscription. A future bridge must explicitly own a
stream, return generation/freshness, stop on owner loss and demonstrate audio/mic
coexistence. Native parser support does not imply dongle motion support.

## Fixtures, attribution and limits

Fixture v1 SHA-256:
`6955f5f7a39b8db636e1a31b15441b2b617c6532b766a12172a74941c4cc461c`.
Locations: NTPods `windows/daemon/tests/fixtures/sensor-wire-v1.json`; LibrePods
`android/core/src/test/resources/sensor-wire-v1.json`.

Public announcement evidence comes from [MousePods](https://github.com/starman-tech/mousepods/tree/79736c79ae5f86d62a137e5747b658431b8c3d9e),
by Matteo Gaudioso (starman-tech), pinned commit
`79736c79ae5f86d62a137e5747b658431b8c3d9e`. Documentation/captures are
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). Modifications: selected
eight announcement frames and added synthetic motion/command vectors. Synthetic
vectors are not hardware observations. Parsers are our implementations of the
documented format, not copied MousePods code. Existing Pro 3 WX evidence remains
in the macOS capture documentation.

Tests cover raw fields, exact commands, read boundaries, malformed/truncated
batches and XML, identity quaternion, invalid axes, model gates, dynamic IDs,
stop/rearm, suspension, failed-stop recovery and stale sockets. No hardware
streaming, new gesture calibration, acoustic quality, latency or simultaneous
microphone benchmark is claimed here.
