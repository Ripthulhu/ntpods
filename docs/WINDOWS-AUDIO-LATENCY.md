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

The AirPods' own buffer is a separate research target. LibrePods names control
`0x0B` [JITTER_BUFFER](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/aacp/types/ControlCommand.kt#L35),
and USBPods defines [90/60 ms profiles](https://github.com/wasdwasd0105/USBPods-Pico2W/blob/6286ab1618ec1143d828b1fc377d8e22c449e567/src/settings.h#L95).
Its public Lite build [stubs out the sender](https://github.com/wasdwasd0105/USBPods-Pico2W/blob/6286ab1618ec1143d828b1fc377d8e22c449e567/src/lite_stubs.c).
Confirm the value semantics, ordering and restoration with captures before
testing writes. NTPods does not write this control.

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
