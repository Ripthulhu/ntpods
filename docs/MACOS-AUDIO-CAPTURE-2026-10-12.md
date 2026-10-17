# Tahoe audio protocol findings

Evidence: AirPods Pro 3 firmware 9A348, Tahoe 26.7.1 (25G241), and the Intel
AX200 VM in the [capture guide](MACOS-PROTOCOL-CAPTURE.md). The 2026-10-12 traces
contain CVSD hands-free capture and Apple AAC-ELD playback. The 2026-10-14 trace
contains an AAC-ELD gaming transition with experimental host eligibility patches.
Neither exercises NTPods' AACP microphone route.

## Capture scope

The October 12 pairing/profile trace has 15,329 frames; the microphone trace
has 12,322. Neither has capture drops or truncated frames. The microphone trace
starts mid-connection and needs the pairing trace's dynamic channel bindings.
Merge them chronologically and force Bluetooth decoding for the verified USB
controller. The first 15,329 merged frames belong to pairing; subtract 15,329
from later frame numbers to locate them in the original microphone trace.

Complete CaseInfo `0x23` and certificate `0x27` packets needed empty-tail guards
in the pinned Lua dissector. HFP and AVDTP decoding was unaffected; see
[capture limits](MACOS-CASE-CAPTURE-2026-10-12.md#capture-quality-and-limits).
Keep raw traces, merged captures and unredacted logs outside Git.

## Microphone and return to playback

Both the Settings input meter and Voice Memos negotiated HFP codec 1 (CVSD).
Successful eSCO connection events report CVSD air mode, a 12-slot interval
(7.5 ms), and 60-byte receive/transmit packets. USB isochronous transfers ran in
both directions. TShark did not decode their audio payloads in this setup;
codec identification comes from HFP negotiation and the HCI connection event.

| Event | UTC | Original microphone frame |
| --- | --- | --- |
| Voice Memos codec selection | 21:27:40.124 | 8238 |
| eSCO connection complete | 21:27:40.603 | 8345 |
| Host requests eSCO disconnect | 21:28:02.797 | 11268 |
| eSCO disconnect complete, local reason `0x16` | 21:28:03.133 | 11294 |
| Later playback Start request | 21:28:15.719 | 11508 |
| Start accepted | 21:28:16.010 | 11513 |
| First subsequent RTP packet | 21:28:16.099 | 11547 |

No AACP `0x58` start, stop or uplink appears. The ACL connection and AAC-ELD
channel survive microphone use; later playback starts without reconfiguration.
Playback was started separately, so the gap before Start does not measure
recovery delay. Simultaneous stereo playback during capture and acoustic channel
separation afterward remain unverified.

## AAC-ELD playback comparison

Pairing frame 5177 requests vendor `0x004C`, codec `0x8001`; frame 5194 accepts
it. All vendor profile bytes match NTPods' template except the bitrate field,
which NTPods' parser reads as 320 kbps with its high bit set. The guest configures
its encoder separately, so that field is not the measured stream bitrate.

| Setting | Tahoe evidence | NTPods at capture time |
| --- | --- | --- |
| Rate and channels | Encoder logs: 48 kHz stereo | 48 kHz stereo |
| Encoder bitrate | Logs: 192 kbps target, VBR, 256 kbps maximum | CBR, selectable 128–256 kbps |
| Access-unit bounds | Logs: 32–320 bytes | At most 321 bytes |
| Packet grouping | Logs: three frames per 30 ms | Two 480-sample frames per 20 ms |
| Transport size | Logs: MTU 1,023, RTP frame length 988 | Bound to negotiated media MTU |
| RTP timestamps after recording | Increments 27–33, usually 30, at roughly 30 ms intervals | Increment 960 sample ticks per 20 ms |
| Reported delay | Earbud DelayReport 150 ms; guest adds 30 ms packet grouping | No equivalent measured comparison in this run |

Guest encoder logs report `delayMode=1` and `mCodecDelay=240` samples. These are
encoder properties, not a measured end-to-end delay. The trace did not establish
the encoder's AudioSpecificConfig, SBR setting or a minimum playback buffer.
The timestamp and grouping differences remain unresolved; this capture does not
establish an error in NTPods' hardware-tested packet format.

## Delay reporting and low-latency mode

Ordinary AVDTP reports delay in big-endian tenths of a millisecond:

| Frame | Direction | Observation |
| --- | --- | --- |
| 5136 / 5164 | Buds to host | AAC and Apple vendor codec capabilities both include Delay Reporting, category `08 00` |
| 5177 | Host to buds | AAC-ELD SetConfiguration includes that category; host SEID is 6 |
| 5204 | Buds to host | DelayReport addresses host SEID 6 and carries `05 DC`: 1,500 units of 0.1 ms, or 150 ms |
| 5215 | Host to buds | Accepts the report, before the Open response at frame 5221 |

macOS adds 30 ms of RTP packet grouping to the 150 ms sink report and exposes
180 ms to the audio system. This does not measure acoustic latency or identify
the adjustable part of the sink delay. The October 12 pairing/microphone traces
contain one report and acknowledgement, with no AACP control `0x0B` or later
report during CVSD/HFP capture or resumed playback.

The unpatched gaming checks report local support `0`, remote support `1`, and
reject dynamic latency. This establishes a host restriction in this VM.
[Apple requires Apple silicon for Mac Game Mode](https://support.apple.com/en-us/105118).

### Working gaming transition

The October 14 capture, with local gaming and dynamic-latency checks patched,
records entry to and exit from gaming mode during audible AAC-ELD playback.
The listener reported clean audio in both transitions.

| Value | Gaming | Restored |
| --- | --- | --- |
| AACP `0x0B` requested buffer, LE32 milliseconds | 60 ms | 150 ms |
| Immediate AVDTP DelayReport, BE16 tenths of a millisecond | 28 ms | 90 ms |
| Completed HAL jitter target | 60 ms | 150 ms |
| HCI Automatic Flush Timeout (`0x0C28`) | 96 slots / 60 ms | 320 slots / 200 ms |
| AAC-ELD frames per RTP packet | Two 10 ms frames | Three 10 ms frames |

Each target command precedes the earbuds' report and controller timeout update.
Both timeout commands succeeded. All 2,632 RTP packets and their access-unit
sequences were continuous, with no AVDTP Suspend or Close.
Gaming packets averaged 19.968 ms apart; the maximum interval was 21.808 ms.
The host requested 22 ms of clock adjustment entering gaming and 60 ms leaving
it, matching the [audio adjustment calculation](#coordinated-audio-adjustment).

All 2,632 packets use RTP `80 60` and SSRC zero. Timestamps advance 68,737
ticks over 68,739 ms of captured traffic. Before gaming, each packet carries
30 ms of audio, but timestamp steps alternate mostly between 20 and 40,
following the same packet's transmission interval rather than its audio length.
This supports an elapsed-host-time clock in this sender. The first timestamp
is 274; the exact clock source and epoch are not established. SSRC zero is an
observed field, not a proven clock selector. See the
[Windows comparison](WINDOWS-AUDIO-LATENCY.md#packet-and-source-reserve-overrides).

The AX200 lacked Apple's vendor priority helpers; no vendor HCI commands were
transmitted. Standard timeout control sufficed for this run. Capture began after
connection setup, so PFC use is unknown. Requested buffer, reported delay and HAL
target are distinct, and none measures acoustic latency. Restoration while idle
or suspended remains unverified.

### Connection setup and RTP clock

A controlled comparison on the same Pro 3 / 9A348, Tahoe 25G241 and AX200
completed ordinary AVDTP SetConfiguration, Open and Start with PFC disabled.
Playback stayed clean for 60.359 seconds: 2,013 packets, three AUs per packet,
SSRC zero and 999.984 RTP ticks/second, with continuous sequence numbers.
Fresh host SDP advertised A000 `00A006C4` (Dynamic Latency bit `0x2000`
clear), A001 `00000000005D040D` and AFFF `1`. The receiver reported 150 ms;
the controller accepted a 200 ms flush timeout. Neither PFC nor that host
A000 bit is required for millisecond timestamps in this setup. The remaining
Apple identity, AACP messages and sender timing have not been fully isolated.

The host DID version (`0203`) is derived from macOS's system product version:
this guest's `26.7` encodes `1A70`. The Windows host advertised `0A00`.
Overriding only the Mac's outgoing version with `0A00` retained clean
millisecond-clock playback for five minutes, including a media suspend/start.
Fresh SDP confirmed `004C:0000:0A00`, vendor source `1`, with the original
Apple feature values. Version `1A70` is not required for this Mac path;
the DID product and other setup differences remain untested.

Zeroing all three SDP values preserved millisecond RTP but suppressed incoming
AACP capability/settings reports. Three connections ended remotely after
31.1–31.4 seconds. Restoring the values restored those reports and 72.7 seconds
of clean, uninterrupted playback. Zeroing only A000 reproduced one initial
31.3-second reconnect, but a later AAC-ELD stream ran for 340.5 seconds with
millisecond RTP, three AUs per packet and no sequence gaps or disconnection.
A000 zero therefore does not consistently prevent sustained millisecond audio.

With A000 zero, the buds also requested a 30 ms signaling/media channel flush
timeout, as seen with Windows; that option was absent with the working values.
These fields affect receiver behavior, but no clock-selection bit or reconnect
timer has been identified. Zero-valued attributes differ from absent ones;
the longer stream's initial setup fell in a capture gap.

Host timing stalls can leave gaps in the Mac's RTP sequence counter while its
encoded AU sequences remain continuous. Check AU continuity before interpreting
an RTP counter gap as missing encoded audio.

For NTPods packet grouping, source reserve and the Windows requirement to reopen
media when changing flush timeout, see the
[Windows policy](WINDOWS-AUDIO-LATENCY.md#automatic-receiver-buffer-policy).

### Buffer-control encoding in the saved daemon

Static inspection of the same guest's `/usr/sbin/bluetoothd`, x86-64 slice,
connects jitter-buffer targets to AACP control `0x0B`. The complete universal
file is 23,555,568 bytes, SHA-256
`93f3afbf2bb7d84855ae1f2b24c9a222250f33c2808a7b80094bae15c0f62f65`.
Addresses below are unslid virtual addresses for this build.

| Address | Observation |
| --- | --- |
| `0x100447EA8` | Chooses a 16-bit target from the mode table at `0x100A79FD2`; unlisted modes use 150 |
| `0x100453C0D`–`0x100453C20` | Reads that target from the audio object's `+0x2C4` field and calls the common sender with control ID `0x0B` |
| `0x10044F839` / `0x10044FD4A` | Converts a reported delay from tenths of a millisecond to milliseconds, then subtracts the same stored target without rescaling; the adjustment log explicitly uses milliseconds |
| `0x1000BE084` | AACP implementation writes opcode `09 00`, the control ID, then four little-endian value bytes |
| `0x1002B366C` | Configuration paths select 150 or 500 under different platform and capability checks |
| `0x100498CCB` → `0x1005A8460` → `0x1000469A6` → `0x100046A58` | Bluetooth path of `GetAdaptiveLatencyJitterBufferLevel` returns a cached host audio-object value at `+0x10`; it sends no read request |
| `0x1002A1DD0` | Incoming control dispatcher routes `0x0B` to `Unsupported control type` at `0x1002A269C` |

The mode table contains 30, 60, 80, 100, 120, 140, 150 and 300 ms; separate
configuration paths use 500 ms. The sender dispatches through a device-specific
implementation. Predicate `0x1002948AA` checks product IDs and firmware in some
paths and is not a complete modern-model support list. Only the 60/150 ms
transition above has capture evidence here.

The serializer produces body `09 00 0B 3C 00 00 00` for 60 ms and
`09 00 0B 2C 01 00 00` for 300 ms, excluding the outer AACP header. These
examples derive from code; 300 ms has no observed write here. Reading only the
first value byte would misread 300 as 44. A usable 30 ms minimum and a universal
150 ms original setting remain unverified.

The cached getter and incoming dispatcher establish no remote readback command.
An AVDTP DelayReport does not identify the AACP target. NTPods'
[temporary buffer trial](WINDOWS-AUDIO-LATENCY.md#earbud-delay-and-buffer-control)
requires an explicit restoration value; a completed send confirms neither
application by the earbuds nor restoration of their original setting.

### Profile Fast Connect delay metadata

PFC A2DP descriptor `0x12` reports delay as LE16 tenths of a millisecond.
Three case/wear connections contain complete PFC setup; frame 927 in the first
carries `12 02 DC 05` (150 ms). See the [PFC reference](APPLE-FAST-CONNECT.md#a2dp-descriptors)
for decoder addresses, byte order, the shared AVDTP latency setter and sources.
NTPods uses ordinary negotiated AVDTP reporting.

A [public Pro 2 text export](https://github.com/BorgoGustavo/airpods-ctl/blob/b42f4e8a944d39fdd9fa25533ae262bf647ddb3b/docs/captures/2026-08-31-open-and-set-mode.txt)
shows PFC discovery, a 200 ms flush timeout, then a PFC-labelled Delay Report
and acceptance before ordinary AVDTP setup. It includes no bytes for that
signal or Game Mode transition.

### Game Mode capability checks

The same saved daemon checks both remote support and its own controller before
publishing low-latency gaming support:

| Address | Condition |
| --- | --- |
| `0x100449735`–`0x100449761` | Requires remote capability `0x31` and the platform check at virtual slot `+0xF8` |
| `0x1004AA8D0`, table `0x1004AAC34` | Remote capability `0x31` reads device byte `+0x46E`, mask `0x10`, with valid capability data at `+0x44D` |
| `0x1004AC44E`–`0x1004AC8BE` | Reads PnP Information service `0x1200`: base features from four-byte SDP attribute `0xA000`, extended features from eight-byte attribute `0xA001` |
| `0x1004AF45C` | Stores the extended-feature word at device `+0x468`; the gaming check reads bit 52 of this stored word |
| `0x1006B3038` / `0x1006B303D` → `0x1006B33E9`–`0x1006B33FC` | Incoming PFC Echo parser reads little-endian words at command offsets `+0x24` / `+0x28`, placing the first in the upper half of the stored word |
| Platform table `0x100ABBDD0 + 0xF8` → `0x1004127F6` | Requires `AudioAccessoryFeatures` / `AoSGaming`, then controller virtual slot `+0xB48` |
| Constructor `0x100339650` | Installs controller table `0x100AB5950`; slot `+0xB48` resolves to `0x10033BBDC` |
| `0x10033BBDC` | Reads chipset enum `+0x334`; accepts 23–1999 or 3000–3999 and rejects zero |

The AX200 uses chipset zero (`THIRD_PARTY_DONGLE`) and fails the unpatched host
check regardless of the feature flag. Advertising an Apple identity to the buds
does not change this classification. The check establishes a guest software
restriction, not an AX200 physical latency limit under Windows.

The Pro 3 pairing capture's frame 4760 contains SDP attribute `0xA001` with
value bytes `00 00 00 00 08 7C 7C 1B`. A separate pairing capture's frame 878
contains PFC words `1B 7C 7C 08` and `00 00 00 00`; the parser above produces
internal value `0x087C7C1B00000000`. Its bit 52 is set, consistent with the
guest's remote gaming support value of `1`.

The SDP uint64 conversion follows the same word ordering. Data-element header
`0B` selects the eight-byte path at `0x10015BC87`; `0x10015BF66` and
`0x10015BF8D` read the two big-endian 32-bit halves and store each in native
byte order. A 64-bit load on x86 therefore rotates the canonical SDP integer
by 32 bits. The captured value becomes `0x087C7C1B00000000`, matching PFC.
Internal bit 52 corresponds to bit 20 (`0x00100000`) of this canonical SDP
value's low half. This mapping is specific to the inspected representation.

Cached/iCloud data can also fill that word. Internal query `0x31` is not bit 49
of the outgoing AACP feature declaration. These checks establish eligibility;
they do not establish the safety of a controller override.

### Coordinated audio adjustment

The dynamic-change path at `0x100453A74` stores the target, sends `0x0B` and
starts a 2,000 ms DelayReport timeout. A report received during the transition
cancels that timer, updates flush settings and compares the reported delay,
converted to whole milliseconds, with the target (`0x10044FD24`).

The audio-layer message at `0x100674F8C` carries
`kBTAudioMsgPropertySamplingSpeedParams` with two fields:

| Field | Value |
| --- | --- |
| `kBTAudioMsgPropertySamplingIntervalMultiplier` | 1.1 when the report exceeds the target; 0.9 when it is below |
| `kBTAudioMsgPropertyLatencyChangeMsec` | Absolute difference, minus 10 ms when the difference exceeds 10 ms and the target is not 150 |

Static inspection of this guest's `BTAudioHALPlugin` establishes the consumer.
The universal file is 1,741,872 bytes, SHA-256
`769658bd9eae5e84d44435b005beb6ecc00eddeaea1621eb3c3c7122252d6571`;
HAL addresses below are unslid addresses in its x86-64 slice. Its
`A2DPAudioDevice` setter at `0x3151E` stores the multiplier and adjustment,
resets progress timing and activates a nonzero adjustment. A separate
`kBTAudioMsgPropertySamplingSpeed` capability boolean gates the setter.

`GetZeroTimeStamp` (`0x51739` -> virtual slot `+0xB0` -> `0x39A2`) multiplies
the host-time interval for a fixed sample-frame quantum by the requested value.
Thus 1.1 slows the reported device clock to 1/1.1 of nominal; 0.9 speeds it to
1/0.9. It preserves the existing timestamp anchor and sample-time continuity.
The setter does not directly schedule packet writes or change a PCM resampler.
[Apple describes the sample-time/host-time pair as controlling HAL I/O timing](https://developer.apple.com/videos/play/wwdc2021/10190/).
This supports a change to the producer's callback cadence through HAL; the
specific resampling behavior inside Core Audio was not reconstructed.

The write path at `0x35BC`–`0x35D9` passes its input frame count and
`mach_absolute_time()` to progress function `0x3292C`. The first write starts
the epoch. Later writes accumulate frames and compare them with nominal
sample rate times elapsed host time. Slowdown finishes when nominal frames
minus submitted frames reaches `adjustment_ms * sample_rate / 1000`; speedup
uses the reverse subtraction. A 10,000 ms limit also forces completion, checked
during writes. Ideal durations are about 11 times the adjustment for 1.1 and
9 times for 0.9; the measured frame difference is the actual stop criterion.

Completion clears the active state and sends XPC message 14 with status true
(`0x32D83` -> `0x317EC` -> `0xEA16`). This also occurs on the time limit, so it
does not prove that the desired drift was achieved. The daemon's completion
handler at `0x100453466` then restores multiplier 1.0 with zero adjustment and
clears its transition flag. The two-second timer only bounds the earlier report
wait. Live transition evidence is limited to the patched October 14 capture.

Controller flush timing is separate. Function `0x10044F910` selects 60 ms for
target 30, the matching timeout for targets 60/80/100/120/140, normally 200 ms
for target 150, and 300 ms for target 300. Platform checks can skip the controller
write. The Windows profile driver's channel-update interface has no timeout field.

Sending `0x0B` alone therefore does not reproduce this host transition. Slowing
NTPods' packet loop without coordinating its Windows PCM producer could move
buffered audio into the source queue or overflow it, rather than reduce total
latency. The command-triggered report may describe the delay before adjustment;
it is not confirmation that the requested target has been reached.

## Intel controller and microphone eligibility

The VM loaded IntelBTPatcher 2.5.1, BlueToolFixup 2.7.2 and Lilu 1.7.2.
Setup records identify IntelBTPatcher as the
[lshbluesky 2.5.1 release](https://github.com/lshbluesky/IntelBluetoothFirmware/releases/tag/v2.5.1),
whose source includes upstream Tahoe changes and later BLE fixes; the running
kext was not independently hashed. IORegistry showed AX200NativeComposite
active. IntelBluetoothFirmware was disabled in the saved config and not loaded;
the passed-through controller already had firmware. Bluetooth NVRAM matched the
[BlueToolFixup instructions](https://github.com/acidanthera/BrcmPatchRAM/blob/2.7.2/README.md#bluetoolfixupkext),
including 14 zero bytes for `bluetoothInternalControllerInfo`.

System Information reported `THIRD_PARTY_DONGLE`. Captured controller logs classify
the external adapter with chipset value zero. Inspection of this build's x86-64
`bluetoothd` shows that its WBS, UWBS and stereo capability checks reject that
value. The UWBS Mac branch identifies supported Broadcom chipsets starting with
BCM4364B3. This explains why the wider-band HFP codecs were unavailable in this
run; it does not establish the eligibility rules for the separate studio-quality
recording feature. The log text `disabled by defaults write` also occurs when
these capability checks fail, without a saved disabling preference.

Hackintosh reports describe a separate
[AirPods playback failure on Intel controllers](https://www.reddit.com/r/hackintosh/comments/1sex75t/release_finally_fixed_the_infamous_airpods/),
including an AX200/Pro 3 test. The proposed
[BlueToolFixup FastConnect patch](https://github.com/acidanthera/BrcmPatchRAM/pull/38)
bypasses a failed Bluetooth clock check during A2DP startup. It does not enable
microphone codecs. This trace demonstrates AAC-ELD playback without it.
[Earlier microphone reports](https://www.reddit.com/r/hackintosh/comments/mab9hp/native_intelbluetoothfirmwarekext_engaging/)
describe firmware-dependent HFP failures and warm-boot workarounds; those tests
predate Tahoe's studio-quality feature.

[Apple's firmware notes](https://support.apple.com/en-us/106340) include Pro 3,
macOS 26 and Voice Memos for studio-quality recording. They do not establish
support for an AX200 VM. Apple's
[high-quality recording API](https://developer.apple.com/documentation/avfaudio/avaudiosession/categoryoptions-swift.struct/bluetoothhighqualityrecording)
documents an EU restriction, but that API lists iOS, iPadOS and Mac Catalyst.
Its restriction alone cannot explain native macOS Voice Memos behavior.

Studio-recording eligibility and its required controller commands remain
unresolved. The HFP capability checks alone do not justify an override.
