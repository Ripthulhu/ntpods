# Tahoe audio protocol findings

Evidence: captured on 2026-10-12 and 2026-10-14 with AirPods Pro 3 firmware 9A348,
Tahoe 26.7.1 (25G241), and the patched Intel AX200 VM
described in the [capture guide](MACOS-PROTOCOL-CAPTURE.md). The October 12 capture used CVSD
hands-free capture and Apple AAC-ELD playback. It did not exercise the AACP
microphone route used by NTPods.

## Capture scope

The first session contains pairing and profile setup; the second contains the
microphone check. They have 15,329 and
12,322 frames respectively, with no capture drops or truncated frames. The
second trace starts mid-connection, so decoding it alone misses dynamic channel
bindings. Analysis merged both traces chronologically and forced Bluetooth
decoding for the verified USB controller. Its first 15,329 frames belong to
the pairing session; subtract 15,329 from later merged frame numbers to locate
them in the original microphone trace.

Complete CaseInfo `0x23` and certificate `0x27` packets needed empty-tail guards
in the pinned Lua dissector. HFP and AVDTP decoding was unaffected; see
[capture limits](MACOS-CASE-CAPTURE-2026-10-12.md#capture-quality-and-limits).

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

No AACP `0x58` start, stop or uplink appears. The ACL connection and configured
AAC-ELD playback channel remain present across microphone use. Later playback
starts on that channel without codec reconfiguration. Playback was started
separately, so the gap before Start is not a recovery-delay measurement. This
run also does not verify simultaneous stereo playback during capture or acoustic
channel separation afterward.

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
The timestamp and packet-grouping differences warrant further decoding; this
capture does not establish that NTPods' hardware-tested packet format is wrong.

Raw traces, merged captures and unredacted logs remain outside Git.

## Delay reporting and low-latency mode

The pairing trace separates delay reporting from buffer control:

| Frame | Direction | Observation |
| --- | --- | --- |
| 5136 / 5164 | Buds to host | AAC and Apple vendor codec capabilities both include Delay Reporting, category `08 00` |
| 5177 | Host to buds | AAC-ELD SetConfiguration includes that category; host SEID is 6 |
| 5204 | Buds to host | DelayReport addresses host SEID 6 and carries `05 DC`: 1,500 units of 0.1 ms, or 150 ms |
| 5215 | Host to buds | Accepts the report, before the Open response at frame 5221 |

macOS logs add 30 ms of RTP packet grouping to the 150 ms sink report and expose
180 ms to the audio system. This accounting does not measure acoustic latency
or identify how much of the sink delay is an adjustable buffer. The merged
pairing/microphone trace contains one report and its acknowledgement; it has no
AACP control `0x0B` or later delay report during microphone use or resumed
playback. That microphone session used CVSD/HFP, not NTPods' AACP uplink.

The guest logs report low-latency gaming support as local `0`, remote `1`, and
reject dynamic latency as unsupported. This establishes a host-side restriction
in this VM; it does not establish the command sequence for entering that mode.
[Apple requires Apple silicon for Mac Game Mode](https://support.apple.com/en-us/105118).
The later transition below used an experimental eligibility patch in the Intel VM.

### Working gaming transition

On 2026-10-14, a capture with the local gaming and dynamic-latency checks patched
recorded a complete transition during audible AAC-ELD playback. The listener
reported clean audio when entering and leaving gaming mode.

| Value | Gaming | Restored |
| --- | --- | --- |
| AACP `0x0B` requested buffer, LE32 milliseconds | 60 ms | 150 ms |
| Immediate AVDTP DelayReport, BE16 tenths of a millisecond | 28 ms | 90 ms |
| Completed HAL jitter target | 60 ms | 150 ms |
| HCI Automatic Flush Timeout (`0x0C28`) | 96 slots / 60 ms | 320 slots / 200 ms |
| AAC-ELD frames per RTP packet | Two 10 ms frames | Three 10 ms frames |

The target command was followed by the earbuds' report, then the controller
timeout update. Both timeout commands succeeded. All 2,632 RTP packets and
their access-unit sequences were continuous, with no AVDTP Suspend or Close.
Gaming packets averaged 19.968 ms apart; the maximum interval was 21.808 ms.
The host requested 22 ms of clock adjustment entering gaming and 60 ms leaving
it, matching the calculation in the next sections.

The AX200 did not support Apple's vendor priority helpers, and no vendor HCI
commands were transmitted. Standard timeout control was sufficient for this
run. The capture starts after connection setup, so it cannot establish whether
PFC was used. Requested buffer, reported delay and completed HAL target are
distinct; none is an acoustic latency measurement. Restoration while idle or
suspended is not established by this active-playback capture.

NTPods retains one-frame AAC-ELD packets and a 20 ms source reserve. Its Windows
profile driver must reopen media to change the flush timeout; the supported
channel-update interface has no timeout field. See the
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

The mode table contains targets 30, 60, 80, 100, 120, 140, 150 and 300; 500
appears in separate configuration paths. These are host implementation values;
the capture does not show the earbuds accepting them. The common sender
dispatches through a device-specific implementation. Predicate `0x1002948AA`
checks specific product IDs and firmware versions for some configuration paths;
it is not a complete modern-model support list. The inspected code establishes
the available AACP encoding and intended units, without proving every model
supports it.

For control `0x0B`, that serializer produces body `09 00 0B 3C 00 00 00` for
60 and `09 00 0B 2C 01 00 00` for 300. A decoder that reads only the first
value byte would misread the latter as 44. These examples exclude the outer
AACP header and are derived from code, not observed writes in this capture.
Neither 30 as a usable minimum nor 150 as every device's original setting is
verified. The cached getter and incoming dispatcher do not establish a remote
readback command, and an AVDTP DelayReport does not identify the AACP target.
NTPods' [temporary buffer trial](WINDOWS-AUDIO-LATENCY.md#earbud-delay-and-buffer-control)
therefore requires an explicit restoration value. A completed send confirms
neither application by the earbuds nor return to their original setting.

### Profile Fast Connect delay metadata

Apple Profile Fast Connect (PFC) exchanges profile configurations and channel
parameters during setup. Apple's [protocol description](https://patents.google.com/patent/US20190373430A1/en)
includes AVDTP delay parameters; [ToothPicker, appendix A.1.2](https://www.usenix.org/system/files/woot20-paper-heinze.pdf)
describes discovery over L2CAP Echo followed by configuration on a dynamic channel.

A [public Pro 2 text export](https://github.com/BorgoGustavo/airpods-ctl/blob/b42f4e8a944d39fdd9fa25533ae262bf647ddb3b/docs/captures/2026-08-31-open-and-set-mode.txt)
shows PFC discovery, a 200 ms flush timeout, then a PFC-labelled Delay Report
and acceptance before ordinary AVDTP setup. It includes no bytes for that
signal or Game Mode transition.

The saved Tahoe daemon above contains a separate PFC A2DP descriptor path:

| Address | Observation |
| --- | --- |
| `0x100454372`, dispatch table `0x100454A1C` | Descriptor key `0x12` selects a 16-bit delay reader at `0x100454547` |
| `0x100454547` | Logs tenths of a millisecond, multiplies by 100, and calls stream-latency setter `0x100482D10` |
| `0x10044F7FA`–`0x10044F85F` | Ordinary AVDTP Delay Report callback converts the same units and calls the same setter |

Three saved case/wear connections contain complete PFC setup. In the first,
frame 927 carries A2DP descriptor `12 02 DC 05`, reporting 150 ms. Nested
descriptor readers use little-endian integers; construction at
`0x1006AE2FD` and helper `0x1004F182E` confirm the byte order. The
[PFC reference](APPLE-FAST-CONNECT.md) documents the exchange, endpoint
selection and Windows API limits. This is delay metadata; NTPods continues
to use ordinary negotiated AVDTP reporting.

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

The captured AX200 uses chipset zero (`THIRD_PARTY_DONGLE`), so it fails this
host check regardless of the feature flag's live value. Advertising an Apple
identity to the earbuds does not change that macOS controller classification.
This explains a software restriction in the guest; it does not establish a
physical latency limit for the AX200 under Windows.

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

Cached/iCloud data can also fill that word. Internal query `0x31` is not
bit 49 of the outgoing AACP feature declaration. These checks establish
eligibility conditions; they do not establish the complete Game Mode transition
or a safe controller override.

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
wait. No live transition was captured in the capability-limited VM.

Controller flush timing is separate. Function `0x10044F910` selects 60 ms for
target 30, the matching timeout for targets 60/80/100/120/140, normally 200 ms
for target 150, and 300 ms for target 300. Platform checks can skip the controller
write. No supported Windows runtime equivalent was established.

Sending `0x0B` alone therefore does not reproduce this host transition. Slowing
NTPods' packet loop without coordinating its Windows PCM producer could move
buffered audio into the source queue or overflow it, rather than reduce total
latency. The command-triggered report may describe the delay before adjustment;
it is not confirmation that the requested target has been reached.

## Intel controller and microphone eligibility

At capture time, the VM loaded IntelBTPatcher 2.5.1, BlueToolFixup 2.7.2 and
Lilu 1.7.2.
Setup records identify IntelBTPatcher as the
[lshbluesky 2.5.1 release](https://github.com/lshbluesky/IntelBluetoothFirmware/releases/tag/v2.5.1),
whose source includes the upstream Tahoe changes and later BLE fixes. This is
release provenance, not an independent hash check of the running kext.
The captured IORegistry showed the AX200NativeComposite personality active.
IntelBluetoothFirmware was disabled in the saved config and was not loaded;
the passed-through controller had running firmware. The captured Bluetooth
NVRAM values matched the [BlueToolFixup instructions](https://github.com/acidanthera/BrcmPatchRAM/blob/2.7.2/README.md#bluetoolfixupkext),
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
