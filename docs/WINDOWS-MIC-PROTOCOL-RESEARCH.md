# AirPods microphone protocol and Windows reliability research

Source review of NTPods 0.1.15 on 2026-10-11. Upstream hardware results are
author reports. Proposed changes still need testing on Intel AX200 / AirPods
Pro 3 / 9A348.

## Sources and revisions

| Source | Pinned revision | What it establishes |
| --- | --- | --- |
| [LibrePods Linux microphone PR 655](https://github.com/librepods-org/librepods/pull/655) | [99b0e580](https://github.com/LuanAdemi/librepods/tree/99b0e580a93f2a26647f89816895ca1e5f70a3ec) | Original control sequence, demux, FFmpeg decoder and Linux recovery implementation. Still open at review time. |
| [LibrePods Android rewrite](https://github.com/librepods-org/librepods/tree/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c) | `34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c` | Matching microphone commands; timestamp field; Android decoder and in-app recording. |
| [Android microphone PR 723](https://github.com/librepods-org/librepods/pull/723) | [5afad4dc](https://github.com/IvanChanPing/librepods/tree/5afad4dc4de5016a57c1e521b27d044d2e738047) | Bounded decoder worker and app-scoped PCM. Author explicitly reports compile/unit checks only. |
| [Linux microphone PR 774](https://github.com/librepods-org/librepods/pull/774) | [96344dae](https://github.com/injkgz/librepods/tree/96344dae683465e4b5ca036b62d1d08c79be9aff) | Independent FDK-AAC/PipeWire implementation; author reports Pro 3 start/stop coexistence. Closed unmerged as a duplicate of 655. |
| [USBPods two-way fork](https://github.com/han-um/USBPods-Pico2W-2-way-audio-implement/tree/d18d083bc52de3143d8d922c4abaae865cc0dc28) | `d18d083bc52de3143d8d922c4abaae865cc0dc28` | Pico 2 W AACP uplink plus AAC-ELD playback; a different Bluetooth and output-codec path. |
| [NTPods upstream](https://github.com/arctumn/ntpods/tree/188e45df20259b616ce155be812086259d1a2488) | `188e45df20259b616ce155be812086259d1a2488` | Baseline used for this fork. |
| [FFmpeg native AAC implementation](https://github.com/FFmpeg/FFmpeg/blob/n7.1/libavcodec/aac/aacdec.c) | `n7.1` | Decoder frame format, ELD frame length, and flush behavior. |

Rechecked on 2026-10-12: NTPods main, Android rewrite and PR 655 still use the
revisions above. PR 655 remains open.

The most-starred LibrePods fork inventory was refreshed through GitHub's fork
API. The leaders remain hexarch (29), beidaoc (17), Tblob18 (15), Chrisbattarbee
(13), and yicareful (13). Their earlier review is in
[WINDOWS-UPSTREAM-REVIEW.md](WINDOWS-UPSTREAM-REVIEW.md). This review found no
newer Windows microphone transport in those popular forks.

## Protocol facts supported by source

The Linux implementations, Android rewrite and Pico fork agree on the following:

| Item | Value |
| --- | --- |
| Transport | AACP over Bluetooth Classic L2CAP PSM `0x1001` |
| Start packet | `04 00 04 00 58 00 00 00 09 00 00 01 82 00 00 00 04 96 00` |
| Stop packet | `04 00 04 00 58 00 00 00 02 00 03 01` |
| Uplink audio discriminator | AACP opcode `0x0058`, subtype `0x0001` |
| SDU layout | 22 bytes before the first AU; each AU has little-endian `u32` timestamp, `u8` length, compressed bytes |
| AudioSpecificConfig | `F8 E6 30 00`: AOT 39 / ER AAC ELD, mono, 48 kHz coding configuration |
| Presentation assumption | 480 decoded samples per AU presented at 64 kHz: 7.5 ms per AU |

The matching definitions are visible in
[Linux demux](https://github.com/LuanAdemi/librepods/blob/99b0e580a93f2a26647f89816895ca1e5f70a3ec/linux-rust/src/bluetooth/aacp_audio.rs),
[Android requests](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/aacp/AACPManager.kt),
and [the second Linux implementation](https://github.com/injkgz/librepods/blob/96344dae683465e4b5ca036b62d1d08c79be9aff/linux-rust/src/audio/eld.rs).

These sources do not establish semantics for every byte of the 22-byte header,
nor a complete start/stop acknowledgement contract. Sending a packet successfully
means the transport accepted it; only subsequent valid PCM demonstrates capture.
After stop, quiet uplink alone does not demonstrate correct stereo playback.

NTPods validates the entire SDU before emitting any AU, rejects zero-length
units and trailing partial records, and checks the known audio header and subtype.
The upstream demux examples can emit a valid prefix before encountering a
truncated tail.

## Coding rate, presentation clock and timestamps

The original
[Linux decoder](https://github.com/LuanAdemi/librepods/blob/99b0e580a93f2a26647f89816895ca1e5f70a3ec/linux-rust/src/audio/eld.rs)
distinguishes 48 kHz coding from 64 kHz presentation. The other Linux decoder
makes the same distinction, and the Pico presents a 64 kHz USB microphone.
NTPods keeps its Windows endpoint at 48 kHz and resamples from the inherited
64 kHz presentation assumption. That converts 480 source samples into roughly
360 output samples per AU, after resampler delay. Four AUs therefore describe
approximately 30 ms of audio, or 1,440 output samples.

The source agreement and existing pitch checks support retaining that conversion.
They do not calibrate the exact device clock, prove every firmware uses it, or
establish long-run drift. The proposed `+180` timestamp / 24 kHz clock
interpretation has no pinned capture trace and remains unverified.

The 480-sample decoder-output check has a firmer basis than the presentation
clock: parsing the ASC yields escape marker 31 followed by 7 (`32 + 7 = 39`), frequency index 3,
channel configuration 1, and `frameLengthShort = 1`. In FFmpeg 7.1,
`decode_eld_specific_config` reads that flag and `aac_decode_er_frame` selects
960 samples then halves the count for ELD. The selected native `aac` decoder
advertises planar float output. A 48 kHz / mono / 480-sample validation therefore
checks the negotiated fixed coding format without asserting the wire clock.

No reviewed implementation uses the wire timestamp to order or conceal audio:

- Linux PRs 655 and 774, and the Pico demux, skip its four bytes.
- The rewrite's
  [MicrophoneFrame](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/data/audio/MicrophoneFrame.kt)
  exposes an unsigned value, but its recording service sends only the AU to the
  decoder.
- PR 723's
  [decoder](https://github.com/IvanChanPing/librepods/blob/5afad4dc4de5016a57c1e521b27d044d2e738047/android/app/src/main/java/me/kavishdevar/librepods/audio/AacEldAudio.kt)
  creates presentation timestamps by adding 7,500 microseconds per input; it does
  not use the parsed device timestamp.

Consequently, duplicate legality, reset conditions, wrap semantics, and a stable
tick rate remain unverified. Collect aggregate delta counts and wrap observations
before using this field to drop packets, reset the decoder or insert silence.
Do not store AU data or microphone recordings in normal diagnostics.

## Stereo coexistence and recovery

PR 655's
[lifecycle](https://github.com/LuanAdemi/librepods/blob/99b0e580a93f2a26647f89816895ca1e5f70a3ec/linux-rust/src/audio/hires_mic.rs#L221-L260)
starts a persistent virtual source, watches recording clients, and keeps that
source present when the feature is disabled during capture. It restarts capture
after two seconds without SDUs. It also schedules an A2DP reset 800 ms after start
and resets after stop. Its restart loop is not bounded in the way NTPods' recovery
must be. Its Linux profile reset and asynchronous scheduling are not a safe
Windows drop-in.

Android rewrite also
[sends STOP and reconnects A2DP](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/devices/AppleDevice.kt#L209-L212).
Its [profile reconnect](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/devices/Device.kt#L311-L336)
disconnects A2DP, waits 500 ms and reconnects. Neither implementation supplies a
different STOP packet or an acknowledgement that confirms stereo playback.

PR 774's author
[reports](https://github.com/librepods-org/librepods/pull/774)
AirPods Pro 3 A3064 on Fedora 44, about 130 AUs per second, no decode failures in a
session, A2DP retained across repeated toggle cycles, and no further microphone
packets after stop. Its code does not perform an A2DP service reset. The report
also identifies WirePlumber's automatic headset switching as a separate source
of playback pauses. This suggests resets are not universal to the AirPods
protocol. It does not identify a Windows equivalent or establish acoustic
channel separation on the Windows test setup.

The Pico author's
[hardware report](https://github.com/librepods-org/librepods/pull/655#issuecomment-4980170555)
describes about a week of Pro 3 simultaneous playback and capture without a
transport reset. Its
[implementation](https://github.com/han-um/USBPods-Pico2W-2-way-audio-implement/blob/d18d083bc52de3143d8d922c4abaae865cc0dc28/src/btstack/btstack_aacp.c)
uses the same start/stop bytes, an Apple Device ID, and a two-second STOP/START
watchdog. It owns the Bluetooth stack and streams AAC-ELD playback itself.
Replacing Windows' AAC playback path or copying the unbounded watchdog would
change more than the microphone driver and is not justified by this evidence.

[WINDOWS-VALIDATION.md](WINDOWS-VALIDATION.md) records that resetting the decoder
and resampler on ear changes did **not** fix reinsertion static. Packets continued
and decode errors remained zero. Stopping when both buds were out, recovering
playback, then starting a fresh stream restored clean speech in the reinsertion
test. Finite, successfully decoded PCM can still be acoustically wrong.

The same validation record shows why quiet uplink cannot replace playback
recovery. After the 21-minute recording, STOP reached quiet in 753 ms but only
the right bud played. Repairing AAP did not help; resetting the audio services
restored both buds. Earlier AudioSink-only reset attempts returned 0/87, and
scoped KS endpoint reconnect attempts returned 0/1460, including an eight-second
wait. The later full service reset passed 21 ordinary stops with an audible gap.

The current [Windows reset](../windows/daemon/src/bt.rs) disables and enables
AudioSink and Handsfree, with waits of 1,000 and 1,500 ms.
[BluetoothSetServiceState](https://learn.microsoft.com/en-us/windows/win32/api/bluetoothapis/nf-bluetoothapis-bluetoothsetservicestate)
removes and installs service drivers. Recorded recovery took about 15.6 seconds,
including those 2.5 seconds of waits. Shortening the waits would leave most of
the delay and has no verified success here. Existing logs separate each service
call from time waiting for the audio-operation lock; check those timings on the
test system before changing the sequence.

The next experiment should reconnect only the paired AirPods' A2DP transport,
leaving its service driver installed. First establish that the playback driver
supports that operation; the previous KS attempt failed. In an opt-in test,
silence PCM, send STOP, confirm the uplink drains, then reconnect that transport
once. Retain the full service reset on failure. Check separate left/right tones
after repeated short captures and a long recording, and verify no microphone
packets remain. Endpoint availability and its reported stereo format cannot
confirm sound reaches both buds. Keep the current recovery until that test passes.

## Conversation Awareness during capture

The PR 655 maintainer
[recommended disabling device Conversation Awareness](https://github.com/librepods-org/librepods/pull/655#issuecomment-4854732405),
not only ignoring host callbacks. Its implementation makes this optional through
`hires_mic_pause_convo`, defaulting to true. When the known value was on, it writes
control `0x28 = 2`, remembers ownership and writes `0x28 = 1` after capture. The
packet is opcode `0x09` plus that control and a little-endian value. Its setter
optimistically updates local state and comments that the AirPods do not echo it.
That echo claim is not sufficient to overwrite NTPods' confirmed device state.

At the 0.1.15 review baseline, NTPods restores host playback volume and skips
ducking while capturing, but does not temporarily disable device `0x28`. This is
a parity gap, not proof that CA caused the reported static.

A safe implementation needs temporary ownership: only override confirmed-on
state, avoid pending user writes, never persist the temporary off value, and
cancel restoration ownership on any explicit user CA command. Restore only the
same device's owned override, after stopping the stream, including failed-start
cleanup. Unknown state must stay unknown. Ordinary cleanup can attempt restore;
an abrupt process or power loss cannot guarantee it. Tests must cover the user
choosing off during capture, user choosing on during capture, failed writes,
disconnect and recovery, and delayed reports. Do not copy an unconditional
off/on pair.

## Buffering and decoder improvements

NTPods upstream
[PR 7](https://github.com/arctumn/ntpods/pull/7)
reported four AUs per SDU and observed roughly 26 ms median, 56 ms p95, 78 ms p99
and 91 ms maximum arrival gaps on Pro 3. The prior 32 ms cushion underrran; its
64 ms watermark also discarded bursts. That motivated the existing 100 ms target
and 160 ms watermark. Reducing them without current measurements would risk
reintroducing crackle. The
[following statistics change](https://github.com/arctumn/ntpods/pull/8)
also distinguishes an inactive interval from a continuously slow producer.

The local receive path already drops an entire invalid SDU, destroys decoder
history and flushes queued PCM. It also resets after a two-second packet gap.
Proposed changes:

1. **Detect missing decoded PCM during an active capture.** At the review
   baseline, `Capturing` has no PCM-progress deadline. Packets can stop, fail
   decode, or produce no output while the state remains capturing. A bounded
   restart must use successful PCM delivery as progress, keep silence and the
   endpoint present, honor wear/capture permission, and latch after its allowed
   attempt. Silent PCM is valid progress; microphone amplitude is not a watchdog.
2. **Validate decoder output before gain and resampling.** The fixed ASC and
   FFmpeg 7.1 native `aac` decoder imply 480 mono float samples at the 48 kHz
   coding rate. Reject an unexpected count, channel count, sample format or rate,
   and reject NaN/infinite samples before applying gain. The inherited shim only
   checks mono and a positive sample count. Its tanh limiter bounds ordinary
   amplitude but does not turn invalid floating point into validated audio.
3. **Keep output timing bounded.** Validate resampler capacity and account for
   queued output before accepting a frame. Continue to discard, rather than
   drain, old decoder/resampler history across capture revocation or a new stream.
4. **Measure latency where it occurs.** Keep separate SDU arrival, valid AU,
   decoded PCM, delivered PCM, driver underrun, queue-drop and recovery counters.
   A nominal 48 kHz decoded rate does not prove the driver delivered every frame.

The
[FFmpeg send/receive contract](https://ffmpeg.org/doxygen/7.1/group__lavc__encdec.html)
allows zero or multiple output frames for one input and requires draining to
`EAGAIN`; elapsed time alone cannot resolve decoder backpressure. The current
shim loops over output, which should be preserved. The
[resampler API](https://ffmpeg.org/doxygen/7.1/group__lswr.html)
provides output-capacity and delay queries, plus gradual compensation if future
measurements demonstrate drift. Adding adaptive clock control before measuring
the error would add an uncalibrated feedback loop.

## Remaining evidence needed

### Activation delay

The installed 0.1.15 log contains four captures with first decoded PCM between
358 and 1352 ms after the START action began. Those measurements exclude capture
polling, receiver scheduling, recovery from a previous recording and the initial
80 ms silence cushion. They are not end-to-end activation measurements.

An idle receiver could wait another 150 ms after its 100 ms receive request.
The 0.1.16 candidate wakes that idle wait when the capture monitor observes a
change. Capture polling still runs every 100 ms and an outstanding receive is
not interrupted. This removes one avoidable scheduling delay; the improvement
has not been measured on hardware.

Starting during a previous capture's recovery also waits for stereo recovery.
The same log shows roughly 16.7–17.1 seconds from STOP through recovery; see
[Stereo coexistence and recovery](#stereo-coexistence-and-recovery).

### Capture reliability

The 0.1.16 candidate implements a five-second decoded-PCM deadline. A stalled
capture closes its PCM gate, uses the existing bounded stop/playback recovery,
then requires an explicit retry. Valid silent PCM renews the deadline. Decoder
output now checks the fixed format, corruption flags and finite float samples;
gain uses a writable frame, and conversion rejects insufficient output space.
These checks do not detect every possible form of audible static.

Validated AU records now also feed aggregate timestamp-delta counters. The log
records forward-tick totals, interval counts, repeats and modular backward steps
alongside arrival-rate statistics. It stores no AU payload or raw timestamps.
The counters do not set a clock rate, drop frames or reset the decoder. Timestamp
units, long-run clock calibration and the temporary CA override remain open.

The source review does not replace single-bud handoff tests, repeated both-bud
reinsertion, case closure during capture, Bluetooth loss, sleep/resume, multiple
recording clients, disabling during a call, or application crash tests. A new
watchdog also needs a test where packets still arrive but decoding makes no
progress, followed by proof that successful capture cannot restart forever.

Long recordings should compare elapsed wall time, decoded frame count, driver
delivered count, underrun/overflow counts and a known acoustic reference. Check
left/right playback during and after capture; OS endpoint presence or an A2DP
label cannot establish stereo separation. Keep the existing Alternative A2DP
driver and the user-selected playback codec configuration throughout these tests.

No reviewed source supplies a verified quieter/faster Windows stop sequence.
The tested playback recovery remains necessary on this Windows setup.
