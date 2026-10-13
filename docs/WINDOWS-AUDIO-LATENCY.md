# Windows audio latency

NTPods sends Windows PCM through its ACX playback driver, the user-mode encoder
and AVDTP transport, then the AirPods' decoder and playback buffer. Measure the
whole path before changing defaults. Buffer capacity alone does not establish
how much audio is waiting.

## Windows requirements

Use Microsoft's [Low Latency Audio guide](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio)
and [ACX streaming documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/acx-streaming)
when changing this path.

- Declare tested processing periods for the supported signal-processing mode.
  Query `IAudioClient3` to verify what Windows exposes and negotiates.
  A shorter period must be requested by an audio client; ordinary clients can
  continue using the default period.
- Preserve packet ownership and completion notifications. Keep each packet
  index paired with its QPC timestamp. Express circuit latency through
  `EvtAcxStreamGetHwLatency`; distinguish PCM handoff from acoustic presentation.
- Register time-sensitive user-mode threads with
  [MMCSS](https://learn.microsoft.com/en-us/windows/win32/procthread/multimedia-class-scheduler-service).
  Registration and [cleanup](https://learn.microsoft.com/en-us/windows/win32/api/avrt/nf-avrt-avrevertmmthreadcharacteristics)
  belong on the same thread. The playback loop, IPC workers and encoder process
  need separate consideration. Increasing timer resolution does not register them.
- For WASAPI clients, use event callbacks and consider Microsoft's Audio-tagged
  real-time work queues. The NTPods daemon reads private driver IOCTLs, so WASAPI
  flags do not change its current polling loop.

The circuits and driver INF audio interfaces publish a RAW 5 ms quantum
(50,000 HNS, zero sample count) with a 4 ms absolute minimum. Runtime and INF
values must match. See Microsoft's
[packet constraints](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ksmedia/ns-ksmedia-_ksaudio_packetsize_constraints2)
and [processing-mode fields](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ksmedia/ns-ksmedia-_ksaudio_packetsize_signalprocessingmode_constraint).

Recorded initialization results after installation and restart:

| Path | Observed period at 48 kHz | Limit |
| --- | --- | --- |
| Shared playback, default and RAW | 480 frames / 10 ms | The live interface had the 5 ms declaration, but Windows did not offer it |
| Exclusive event-driven PCM16 playback | 240 frames / 5 ms accepted | Initialized and queried without starting audio; sustained streaming unverified |
| Microphone, default and RAW | 240 frames / 5 ms accepted | Silent initialization only |

Publishing a constraint does not prove that the shared engine negotiates it.
Tests compare the full INF property bytes with the circuit declaration and check
short-packet delivery and clock arithmetic.

## Implementation

1. AAC assembles an available 4096-byte frame in at most three reads without
   sleeping between them. Empty reads yield, and each read checks that the
   Windows producer has not restarted before accepting its samples.
2. The playback loop, encoder IPC threads and both encoder workers register for
   MMCSS Audio scheduling. Each thread reverts its registration during cleanup;
   an unavailable scheduler leaves playback working at its ordinary priority.
3. Both ACX circuits publish the complete packet constraint structure, including
   the RAW 5 ms entry. Tests cover its values, short packet delivery and clock
   arithmetic. Shared playback still exposes only 10 ms; exclusive initialization
   accepts 5 ms. Shared-mode negotiation and streaming stability remain unresolved.
4. A stream-end diagnostic summarizes accepted frames, queue depth, deadline
   lateness and media-send duration. It contains no audio or device identifiers.
5. The shared stream engine reports zero hardware FIFO bytes instead of the
   sample driver's invented 128. Microsoft's
   [latency callback](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/acxstreams/nc-acxstreams-evt_acx_stream_get_hw_latency)
   describes circuit delay. Additional delay remains unavailable: neither bridge
   carries correlated remote audio timestamps. Zero here does not establish zero
   Bluetooth latency. Queue occupancy and the source reserve must not be added
   together as if they were separate fixed delays.

The source reserve remains 50 ms. Retest it against 30 ms with fixed mic placement,
alternating baseline/trial starts and loaded-system checks before reducing it.
The 100 ms ring capacity is a separate safety bound.

The sample-count frame clock preserves AAC's fractional 21.333 ms period.
Keep that arithmetic. AAC-ELD groups two 10 ms access units; a single-unit packet
trial needs separate peer compatibility testing. Changing the Windows period
does not change either codec's frame size.

## Earbud delay and buffer control

The [Tahoe capture](MACOS-AUDIO-CAPTURE-2026-10-12.md#delay-reporting-and-low-latency-mode)
contains an earbud-reported delay of 150 ms. Its guest reports low-latency gaming
support as local `0`, remote `1`, but dynamic latency is unavailable. The capture
does not establish a minimum buffer or a working low-latency transition.

NTPods requests `GET_ALL_CAPABILITIES`, falls back to `GET_CAPABILITIES` when
unsupported, and negotiates Delay Reporting category `08 00` when offered.
It acknowledges valid `0x0D` reports addressed to the local SEID, including
reports received while waiting for Open. IPC exposes negotiation through
`playback_delay_reporting` and the latest report through
`playback_delay_tenths_ms`. The latter preserves the big-endian 16-bit value in
0.1 ms units; null means no report. It describes earbud-reported delay, not the
whole Windows-to-sound path or the current AACP buffer target. See
[session.rs](../windows/daemon/src/playback/session.rs) and
[peer_response](../windows/daemon/src/playback/protocol.rs).

LibrePods names AACP control `0x0B`
[JITTER_BUFFER](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/aacp/types/ControlCommand.kt#L51).
The [saved macOS sender](MACOS-AUDIO-CAPTURE-2026-10-12.md#buffer-control-encoding-in-the-saved-daemon)
connects millisecond targets to this control and has an AACP implementation
that writes a four-byte little-endian value. Its host mode table includes
discrete targets from 30 to 300, and separate configuration paths send 500.
The inspected macOS getter reads cached host state; no remote read request was
established. Neither 150 as a universal default nor a reported AVDTP delay as
the original buffer target is justified.
USBPods defines [90/60 ms gaming profiles](https://github.com/wasdwasd0105/USBPods-Pico2W/blob/6286ab1618ec1143d828b1fc377d8e22c449e567/src/settings.h#L90),
but also changes the host queue, startup priming and ELD bitrate. Its
[playback code](https://github.com/wasdwasd0105/USBPods-Pico2W/blob/6286ab1618ec1143d828b1fc377d8e22c449e567/src/btstack/btstack_avdtp_source.c#L1426)
defers an active-stream jitter change until 500 ms after Start is accepted;
the comment records crashes when changing it immediately before suspension or
a bitrate change. The public Lite build
[stubs out the sender](https://github.com/wasdwasd0105/USBPods-Pico2W/blob/6286ab1618ec1143d828b1fc377d8e22c449e567/src/lite_stubs.c).
These profiles do not isolate the buffer's effect or verify a universal default.

A developer's [macOS Game Mode logs](https://developer.apple.com/forums/thread/764070)
show a reported latency change from 151 to 60 ms, a 60 ms flush timeout and
temporary audio delivery at 1.1 times normal speed. The post supplies logs,
not packet bytes or an Apple protocol specification. A transition during live
playback may therefore need coordinated queue and transport changes.

The diagnostic IPC command `set_playback_buffer` runs an opt-in trial without
changing the UI or saved defaults. Its implementation is in
[buffer.rs](../windows/daemon/src/playback/buffer.rs):

- `target_ms` accepts 60, 80, 100, 120 or 140. `restore_ms` must be supplied
  explicitly, must exceed the target, and accepts 80, 100, 120, 140, 150, 300 or
  500. It is the operator's chosen restoration value, not a measured baseline.
- Native playback must be active, Start must have been accepted at least
  500 ms earlier, and the current session must have received a delay report.
  The mic must be idle. Codec, bitrate and manual mic changes are rejected during
  a trial. Opening a recording client ends the trial before mic startup.
- The trial runs for 30 seconds after the initial settling interval, or ends
  earlier on cancellation, stream stop or connection changes. Sending
  `target_ms: null` cancels it. IPC publishes the requested target in
  `playback_buffer_target_ms` and errors in `playback_buffer_error`.

A worker sends AACP `0x0B` with a one-second I/O timeout, then attempts the
supplied restore value even if the first send fails. Local stop and teardown
wait for that attempt before closing the transport. There is a 500 ms quiet
interval after each write, including before local Suspend; this precaution
has not been validated on hardware. A successful restore send does not confirm
that the earbuds applied it, and a lost connection can prevent restoration.
After a restore-send failure, mic and codec changes stay blocked until the
control connection closes. Disconnect and reconnect before another trial;
reconnecting alone does not prove the earbuds restored their original target.

With a test build running and audio playing, the
[trial client](../windows/playback/test-buffer.ps1) displays delay reports and
requests restoration when it exits. It plays no audio and does not adjust volume:

```powershell
pwsh -File windows/playback/test-buffer.ps1 -TargetMs 60 -RestoreMs 150
```

Here 150 is an explicit chosen restore target, not a discovered default. Use
the baseline intended for the test. The backend also expires the trial if the
client closes. A backend crash or lost Bluetooth link can prevent restoration.

The trial keeps the same Windows output, codec, bitrate, packet grouping,
50 ms source reserve and 200 ms media flush timeout. The flush timeout limits
packet lifetime; it does not impose a fixed 200 ms delay. No acoustic latency
saving or model compatibility is established for this control. Compare device
reports with acoustic measurements, then check dropouts under load and
microphone recovery before choosing a default.

## Measurement

Compare timestamped Windows loopback with a wired microphone beside an AirPod.
Use each capture packet's first-sample QPC value, which
[WASAPI returns in 100 ns units](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer).
Reject clipping, bad timestamps, discontinuities within markers and weak matches.
The wired capture path contributes an uncalibrated offset. Use unique markers
for longer tests so repeating tones cannot hide a whole-period delay.

Record queue age, encode/send time, negotiated period and missed deadlines
alongside acoustic results. Use Microsoft's
[WPR media tracing procedure](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/optimizing-windows-devices-for-multimedia-experiences-exercise-2)
to investigate glitches. Keep recordings and device identifiers outside the repo.

Run sustained playback under load and with the high-quality mic active. Check
mic stop/recovery, repeated playback starts, sleep/resume and both local Pro
generations before making a smaller reserve the default. Measurements are in
the [validation record](WINDOWS-VALIDATION.md#playback-latency-investigation).
