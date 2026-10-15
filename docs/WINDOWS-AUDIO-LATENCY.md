# Windows audio latency

NTPods sends Windows PCM through its ACX playback driver, the user-mode encoder
and AVDTP transport, then the AirPods' decoder and playback buffer. AAC-ELD is
the default codec when no preference is saved, with AAC fallback for pairs that
lack a supported AAC-ELD profile. Buffer capacity alone does not establish how
much audio is waiting; a latency claim needs measurement of the whole path.

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
  flags do not replace its private PCM notification request.

The circuits and driver INF audio interfaces publish a RAW 5 ms quantum
(50,000 HNS, zero sample count) with a 4 ms absolute minimum. Runtime and INF
values must match. See Microsoft's
[packet constraints](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ksmedia/ns-ksmedia-_ksaudio_packetsize_constraints2)
and [processing-mode fields](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ksmedia/ns-ksmedia-_ksaudio_packetsize_signalprocessingmode_constraint).

### Automatic period request

When an app starts playback through the native AirPods output, NTPods requests
a supported 5 ms shared-engine period using
[`IAudioClient3::InitializeSharedAudioStream`](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient3-initializesharedaudiostream).
It selects the endpoint by the current pair's Bluetooth identity and native
driver, uses its existing mix format, and checks the negotiated period. A faster
supported period is retained. Unsupported periods, exclusive use and other
initialization failures leave ordinary playback working.

The client stays initialized without starting or submitting audio. This keeps
the request separate from the driver's running-stream count. NTPods releases it
when real playback stops or the playback session ends, then tries again on the
next start. A changed driver stream generation also renews the request, so a
stop/start between status checks cannot retain an invalid client. It does not
change the default output, volume or routing, and does not force a previous
period when releasing the client. Releasing it while idle also avoids retaining
a shared connection that could obstruct
[exclusive-mode access](https://learn.microsoft.com/en-us/windows/win32/coreaudio/exclusive-mode-streams).

With installed 0.2.35, read-only queries confirmed 5 ms during Pro 3 playback
with both codecs and after microphone recovery. Pause/resume listening checks
also passed; see the [hardware results](WINDOWS-VALIDATION.md#playback-latency-investigation).
Disconnects, sleep/removal and exclusive-mode takeover still need hardware
checks. Other clients can change the engine period after a successful query.

### Manual checks

Shared-mode checks with the 0.2.31 app and installed 0.2.29 native drivers:

| Path | Observed period at 48 kHz | Limit |
| --- | --- | --- |
| Shared playback, default and RAW, mix and PCM16 formats | 240 frames / 5 ms accepted | The exposed range was 240–480 frames; ordinary playback used the 480-frame default |
| Exclusive event-driven PCM16 playback | 240 frames / 5 ms accepted in the earlier check | Initialized and queried without starting audio; sustained streaming unverified |
| Microphone, default and RAW | 240 frames / 5 ms accepted in the earlier check | Silent initialization only |

Earlier shared render checks exposed only 480 frames despite the constraint.
That endpoint accepted 240-frame initialization; the exact change that
made it available was not isolated. `GetCurrentSharedModeEnginePeriod` reported
240 while the initialized client existed and 480 after release. The 240-frame
client had a 528-frame WASAPI capacity; capacity is not another fixed delay.
Publishing a constraint alone still does not prove successful negotiation on
another PC. Tests compare the full INF property bytes with the circuit
declaration and check short-packet delivery and clock arithmetic.

Use [test-audio-periods.ps1](../windows/playback/test-audio-periods.ps1) from a
source checkout with VS2022 C++ tools. Supply the exact Windows render endpoint
ID; the probe does not select or change the default output.

```powershell
# Query supported and current periods, without opening a stream.
./windows/playback/test-audio-periods.ps1 -EndpointId '<render-endpoint-id>'

# Request 5 and 10 ms in default/RAW mode, without starting audio.
./windows/playback/test-audio-periods.ps1 -EndpointId '<render-endpoint-id>' -Initialize

# Keep a silent shared client at 5 ms for 30 seconds while other apps play music.
./windows/playback/test-audio-periods.ps1 -EndpointId '<render-endpoint-id>' -HoldSeconds 30
```

After a driver timing change, run `-Initialize` while the earbuds have reported
their delay. Check each supported probe's `initialize` result for `0x00000000`;
the tool's exit code alone does not establish successful initialization.
Default and RAW 10 ms playback must still initialize when 5 ms is unavailable.

The hold accepts 1–120 seconds. It supplies silence to the existing mixer, with
no new audio device, routing or volume change. It checks the active period at
start and end, then stops and releases its client. It reports the period after
release rather than forcing other apps back to the prior value. In the Pro 3
check, the engine remained at 5 ms immediately after release and returned to
10 ms in a later query. The listener heard clean AAC-ELD playback during the
30-second hold. This establishes a short streaming check, not acoustic latency
or stability under sustained load. `-SelfTest` checks the tool without hardware.

## Implementation

1. AAC assembles an available 4096-byte frame in at most three reads without
   sleeping between them. Empty reads yield, and each read checks that the
   Windows producer has not restarted before accepting its samples.
2. The playback loop, encoder IPC threads and both encoder workers register for
   MMCSS Audio scheduling. Each thread reverts its registration during cleanup;
   an unavailable scheduler leaves playback working at its ordinary priority.
   The encoder reader and driver PCM notifications wake playback when work is
   ready. Otherwise playback waits until its next frame or control deadline.
   Old drivers, notification failures and active clock adjustments retain the
   1 ms polling fallback. Control checks remain bounded to 20 ms. This removes
   idle polling without changing codec frame sizes or establishing an acoustic
   latency gain. The [notification contract](../windows/drivers/mic/README.md#shared-playback-bridge)
   covers cancellation and driver compatibility.
3. Both ACX circuits publish the complete packet constraint structure, including
   the RAW 5 ms entry. Tests cover its values, short packet delivery and clock
   arithmetic. The backend requests 5 ms while an app uses the native output;
   if Windows rejects it, the existing shared-engine period remains in use.
4. A stream-end diagnostic summarizes accepted frames, queue depth, deadline
   lateness and media-send duration. It contains no audio or device identifiers.
5. The shared stream engine reports zero hardware FIFO bytes instead of the
   sample driver's invented 128. Microsoft's
   [latency callback](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/acxstreams/nc-acxstreams-evt_acx_stream_get_hw_latency)
   describes circuit delay. Both circuits return zero delay. Publishing the
   AVDTP receiver delay through this callback can make Windows reject playback
   initialization with `AUDCLNT_E_BUFFER_SIZE_ERROR`; receiver reports remain
   available through IPC. Zero here does not mean zero end-to-end latency.
   Queue occupancy and the source reserve must not be added
   together as if they were separate fixed delays.

The source reserve defaults to 20 ms for AAC-ELD and 30 ms for AAC, chosen after
codec negotiation so an AAC fallback keeps its larger reserve. Installed 0.2.35
passed short Pro 3 playback, pause/resume and microphone checks with one-frame
AAC-ELD packets and the 20 ms reserve; see the
[hardware results](WINDOWS-VALIDATION.md#playback-latency-investigation).
Long sessions, loaded-system checks and other models remain unverified for this
combination. These listening results do not measure a latency reduction.
The 100 ms ring capacity is a separate safety bound.

The sample-count frame clock preserves AAC's fractional 21.333 ms period.
Keep that arithmetic. AAC-ELD sends one 10 ms access unit per packet. Changing
the Windows period does not change either codec's frame size.

### Packet and source-reserve overrides

Process environment values allow controlled playback comparisons:

| Variable | Default | Diagnostic override | Effect |
| --- | --- | --- | --- |
| `NTPODS_ELD_AUS_PER_PACKET` | `1` | `2` | Group two 10 ms AAC-ELD frames instead of sending each frame immediately |
| `NTPODS_PLAYBACK_RESERVE_MS` | AAC-ELD: `20`; AAC: `30` | `20`, `30`, `50` | `20` requires AAC-ELD; `30` and `50` work with either codec |
| `NTPODS_ELD_RTP_CLOCK` | `48000` | `1000` | Use millisecond units for AAC-ELD RTP timestamps |

Invalid values reject playback. Values are read when the backend opens a playback
session; they are not saved in app settings. Codec parameters, bitrate preferences
and Bluetooth flush settings stay unchanged. The selected packet count is
included in MTU checks on initial setup and every media reopen.
The reserve applies to both wall-clock and driver-clock scheduling, including
pause/resume and microphone recovery.

Quit NTPods from its tray menu first; closing its window only hides it. From
PowerShell 7.4 or later, launch the app with one change at a time:

```powershell
$app = 'C:\Program Files\NTPods\winui\ntpods-winui.exe'

# Compare two-frame AAC-ELD packets at a 30 ms reserve.
Start-Process -FilePath $app -Environment @{
    NTPODS_ELD_AUS_PER_PACKET = '2'
    NTPODS_PLAYBACK_RESERVE_MS = '30'
}

# After quitting that session, compare the larger reserve separately.
Start-Process -FilePath $app -Environment @{
    NTPODS_ELD_AUS_PER_PACKET = '1'
    NTPODS_PLAYBACK_RESERVE_MS = '50'
}

# After quitting, compare AAC-ELD timestamp units with the default packet/reserve settings.
Start-Process -FilePath $app -Environment @{
    NTPODS_ELD_RTP_CLOCK = '1000'
}
```

The child app passes these values to its backend. Check the startup diagnostic
in `%LOCALAPPDATA%\NTPods\daemon.log` to confirm the selected values. To restore
the defaults, quit and open NTPods normally outside the override process.
No user or machine environment variables are written. An explicit `20` override
rejects an AAC fallback; omit it to use the codec's default.
Completing an earbud-buffer trial does not clear these process values; its
restoration covers only the earbud target, clock adjustment and Bluetooth flush.

At `1000`, each 480-sample AAC-ELD frame advances the RTP timestamp by 10 instead
of 480. Both choices derive timestamps from encoded sample counts, wrap at 32 bits
and preserve the same audio, packet cadence and reset behavior. The selection
survives media reopens and microphone recovery. AAC retains its 90 kHz clock,
including when selected as a fallback.

The [macOS capture](MACOS-AUDIO-CAPTURE-2026-10-12.md#aac-eld-playback-comparison)
and [USBPods analysis](https://github.com/wasdwasd0105/USBPods-Pico2W/blob/6286ab1618ec1143d828b1fc377d8e22c449e567/aac-eld-apple.md#rtp-timestamp-is-millisecond-based-not-sample-based)
show approximately millisecond-based AAC-ELD timestamps. This comparison changes
the units only; it does not reproduce macOS's varying wall-clock increments.
The 48 kHz default has passed playback checks. The 1 kHz setting still needs
hardware validation and does not establish Game Mode support or lower latency.

Keep music, codec, bitrate and volume consistent between comparisons. Check
sustained playback, pause/resume, volume and microphone stop/recovery; repeat
under system load and on Pro 1 and Pro 3 where supported. Single-frame packets
double the packet rate relative to two-frame packets and may affect radio scheduling.

## Earbud delay and buffer control

The [working Tahoe gaming capture](MACOS-AUDIO-CAPTURE-2026-10-12.md#working-gaming-transition)
shows a 60 ms buffer request and controller timeout during active AAC-ELD
playback, followed by restoration to a 150 ms target and 200 ms timeout.
Immediate earbud reports were 28 and 90 ms. The patched AX200 VM completed
both transitions without stopping playback. This establishes the command
sequence on Pro 3, not a minimum buffer or acoustic latency.

NTPods requests `GET_ALL_CAPABILITIES`, falls back to `GET_CAPABILITIES` when
unsupported, and negotiates Delay Reporting category `08 00` when offered.
It acknowledges valid `0x0D` reports addressed to the local SEID in Configured,
Open and Streaming states, including reports received while waiting for Open
or Start. [AVDTP 1.3, section 8.19](https://www.bluetooth.org/DocMan/handlers/DownloadDoc.ashx?doc_id=260860)
requires the initial report after configuration, before Open. IPC exposes
negotiation through
`playback_delay_reporting` and the latest report through
`playback_delay_tenths_ms`. The latter preserves the big-endian 16-bit value in
0.1 ms units; null means no report. It describes earbud-reported delay, not the
whole Windows-to-sound path or the current AACP buffer target. See
[session.rs](../windows/daemon/src/playback/session.rs) and
[peer_response](../windows/daemon/src/playback/protocol.rs).

[Apple Profile Fast Connect](APPLE-FAST-CONNECT.md#a2dp-descriptors)
supplies receiver-delay metadata during profile setup. Three decoded Pro 3
connections carry a 150 ms delay; macOS stores it in the same field as AVDTP
DelayReport. Those traces contain no subsequent audio or Game Mode transition
and establish no additional buffer-control command.

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
a temporary sampling-speed parameter of 1.1. The post supplies logs, not packet
bytes or an Apple protocol specification. The
[saved daemon and HAL implementation](MACOS-AUDIO-CAPTURE-2026-10-12.md#coordinated-audio-adjustment)
use a finite host-clock adjustment. Multiplier 1.1 lengthens the interval for a
fixed sample quantum, slowing the source while the earbuds drain their buffer.
HAL compares nominal elapsed samples with submitted samples to determine when
the requested adjustment is complete. It then notifies the daemon, which
restores multiplier 1.0. This is not evidence of faster packet delivery.

NTPods applies one render-clock mapping to ACX packet scheduling, presentation
position and daemon frame deadlines. The microphone retains its normal clock.
The adjustment starts at a PCM packet boundary and ends when the difference
between completed frames and nominal elapsed frames reaches the requested drift.
A local ten-second limit restores the normal rate even if the daemon stops
responding. Expiry is reported as a failure, not achieved drift. Cancellation,
producer changes and provider closure preserve the accumulated clock offset, so
presentation position does not jump backward.

The Windows Bluetooth API accepts media flush settings at channel open; its
documented update request cannot change them during playback. The inspected
BthPort implementation programs the controller timeout and marks finite-timeout
media flushable, while protecting reliable traffic when the controller supports
nonflushable packets. See the [driver contract](../windows/drivers/aap/README.md)
for the versioned requests and inspected Windows build. NTPods closes and
reconfigures the AVDTP media stream to change this timeout, preserving signaling
and the Windows audio device. Playback briefly stops at each reopen.

### Automatic receiver-buffer policy

NTPods requests a 60 ms receiver buffer while native playback is active and the
mic is idle. It reads the paired device's cached SDP PnP record once per playback
session: bit 20 of the 64-bit Apple attribute `0xA001` enables this policy for
AAC-ELD. A cleared bit disables it. Missing or invalid records retain the existing
Pro 3 (A3063/A3064/A3065) fallback; Pro 3 also retains its tested AAC path.
The query uses the Windows cache without starting an over-air SDP lookup.

Application requires negotiated Delay Reporting, a reported delay above 60 ms,
compatible clock/flush drivers and at least 500 ms since Start. Unsupported models
and streams already reporting a shorter delay keep their receiver settings.

The policy uses the coordinated transition described below: reopen with a
60 ms flush timeout, send the AACP target, then make a finite host-clock
adjustment. Once that adjustment completes, the clock runs normally while the
target remains active. There is no 30-second expiry. Each apply or restore
transition has a 15-second deadline; the driver independently limits a clock
adjustment to ten seconds.

Pause, mic demand and playback-setting changes restore an explicit 150 ms target
and the normal 200 ms flush timeout before playback is re-primed. Keep-active
and pending manual microphone demand prevent automatic application. One worker
waits for restoration outside the command reader and preference locks. Stop,
disconnect and volume commands remain available; another start or route change
must wait. Newer stop or disconnect requests supersede the pending command.
Playback can apply the policy again after a successful restoration. If that
configuration reports zero, a session that previously completed the automatic
adjustment may try again. The new attempt
still needs a fresh nonzero report before changing the clock. An initial zero
does not qualify, and a known delay of 60 ms or less clears that eligibility.
A manual buffer trial disables automation for that playback session.

If applying the policy fails, NTPods attempts restoration and continues ordinary
playback only after the transport is restored and re-primed. Automation stays
disabled for that session. An uncertain media reopen or failed restoration
stops playback with an error instead of repeatedly reconnecting.

The 150 ms restore value is a policy choice, not a readback of the original
receiver setting. Earlier 120 ms Pro 3 checks cover automatic pause/resume
transitions with the 20 ms AAC-ELD reserve and restoration before microphone
capture; see the [hardware results](WINDOWS-VALIDATION.md#earbud-buffer-control).

With NTPods 0.2.40 on Windows, Pro 3 firmware 9A348 and Intel AX200, the listener
confirmed clean AAC-ELD playback in both ears with unchanged volume. The automatic
60 ms request was logged; no new end-to-end acoustic latency measurement was
made. Microphone transitions under the 60 ms policy, sustained use, loaded-system
behavior and other models still need hardware checks.

### Buffer trial

The diagnostic IPC command `set_playback_buffer` runs an opt-in trial without
changing the UI or saved defaults. Both the updated AAP and playback drivers are
required. It first restores any automatic receiver-buffer policy and disables
automation for that playback session. Read-only capability checks reject an
older driver before changing the stream. The implementation is in
[buffer.rs](../windows/daemon/src/playback/buffer.rs):

- `target_ms` accepts 60, 80, 100, 120 or 140. `restore_ms` must be supplied
  explicitly, must exceed the target, and accepts 80, 100, 120, 140, 150, 300 or
  500. It is the operator's chosen restoration value, not a measured baseline.
- Native playback must be active, Start must have been accepted at least
  500 ms earlier, and the current session must have received a delay report.
  The mic must be idle. Codec, bitrate and manual mic changes are blocked during
  a trial. Opening a recording client requests restoration before mic startup.
- The media channel reopens with a flush timeout matching the trial target.
  After Start and a 500 ms settling interval, NTPods drains queued signaling
  before sending AACP `0x0B`. It then waits up to two seconds for another
  DelayReport. A repeated numeric value still counts as a new report.
- The reported delay selects interval multiplier 1.1 or 0.9 and a finite drift
  budget using the captured Apple calculation. The same process runs for the
  supplied restore target. Missing reports, unexpected clock resets and expiry
  end the trial with an error.
- Restoration begins after the 30-second trial limit, or earlier on cancellation,
  capture demand or connection changes. Once its clock adjustment finishes and
  the control-write settling interval has passed, the media channel reopens at
  its normal 200 ms flush timeout. The encoder and PCM queues are re-primed before
  microphone startup is allowed.

Coordinated target and restore writes wait for an empty signaling read before
saving the report counter. Each poll handles at most eight packets; a full batch
defers the write so signaling cannot monopolize playback. This prevents a report
already queued in Windows from satisfying the next wait. AAP writes and AVDTP
reports have no shared request ID, so a later report still cannot prove that
the target was applied.

The playback thread owns the transition. On stream stop or teardown it cancels
the clock adjustment and attempts the supplied earbud restore value before
closing AAP; it does not wait for another report or reopen a departing stream.
A failed target write still triggers a restore attempt. A restore failure blocks
further trials and microphone startup until the connection is closed. The driver
independently resets its clock on provider loss. A backend crash or lost Bluetooth
link can still prevent the earbud restore command from arriving.

IPC publishes the requested target in `playback_buffer_target_ms` and failures
in `playback_buffer_error`. Sending `target_ms: null` cancels the trial. With a
test build running and audio playing, the
[trial client](../windows/playback/test-buffer.ps1) displays delay reports and
requests restoration when it exits. It plays no audio and does not adjust volume:

```powershell
pwsh -File windows/playback/test-buffer.ps1 -TargetMs 60 -RestoreMs 150
```

Here 150 is an explicit chosen restore target, not a discovered default. The
trial keeps the same Windows output, codec, bitrate, packet grouping and selected
source reserve (AAC-ELD: 20 ms; AAC: 30 ms by default). A flush timeout limits
packet lifetime; it is not a fixed delay.
Coordinated AAC and AAC-ELD trials passed short Pro 3 listening checks; see the
[hardware results](WINDOWS-VALIDATION.md#earbud-buffer-control) for versions and
conditions. The listening checks establish neither a numerical latency reduction
nor exact earbud-buffer restoration.

### Playback clock interface

The exclusive playback provider handle exposes two additional versioned IOCTLs.
Existing PCM, status and provider requests retain their layouts. All integer
fields are little-endian. Definitions are in
[RenderPipe.h](../windows/drivers/mic/Common/RenderPipe.h).

| Request | Input | Output |
| --- | --- | --- |
| `0x0022A40C`, set clock | Four `u32`: version 1, render generation, interval in tenths, drift in ms | None |
| `0x00226410`, query clock | None | 64-byte version 1 clock snapshot |

Intervals 9 and 11 accept a drift of 1–500 ms and require one running render
stream. Interval 10 with zero drift cancels continuously. A generation mismatch
rejects the write. The snapshot reports real and virtual time in 100 ns units,
requested and effective intervals, completed and nominal frame counts, expiry,
and phase: idle (0), armed (1), active (2), completed (3), expired (4) or cancelled
(5). Packet completion timestamps remain real QPC timestamps.

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
generations. Measurements are in the
[validation record](WINDOWS-VALIDATION.md#playback-latency-investigation).
