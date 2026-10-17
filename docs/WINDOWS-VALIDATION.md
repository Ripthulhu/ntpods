# Windows validation

Desktop checks used Windows 11 x64, an Intel AX200, AirPods Pro 3 (A3063/A3064,
firmware 9A348) and AirPods Pro 1 (A2084, firmware 6F21). The results below identify
the tested versions and playback driver. Other models in the
[model catalog](WINDOWS-MODEL-SUPPORT.md) have implementation support, but no
recorded hardware pass.

## Native playback driver and bitrate controls

The 0.2.11–0.2.15 checks used NTPods' AudioSink driver. Both codecs use the paired
AirPods output. They do not depend on Alternative A2DP.

| Check | Pro 3 | Pro 1 |
| --- | --- | --- |
| AAC, 256 kbps | Clean left/right playback | Clean left/right playback |
| AAC-ELD, 128, 192 and 256 kbps | Clean left/right playback | Clean left/right playback |
| Continuous AAC-ELD, 256 kbps | 70 seconds without audible dropouts | 70 seconds without audible dropouts |
| Windows volume, mute and codec changes | Software controls worked; output identity and Windows default roles retained | Codec and bitrate playback checks passed |
| Stereo during a 20-second microphone capture and after stopping | Both codecs passed listener checks | High-quality microphone disabled for this model |
| AAC-ELD Variable mode | Clean stereo and pause/resume (0.2.32 preview and installed 0.2.33, 256 kbps peak) | Untested |

The 0.2.28 Pro 3 check exposed missing volume synchronization: stem gestures
changed the earbuds' gain and opened the Windows volume indicator, but its
value stayed unchanged. Earlier Windows volume checks did not cover this
direction. With 0.2.29 after reboot, stem gestures updated both the Windows
indicator and backend volume reports. Windows volume changes audibly adjusted
AAC music, and mute/unmute silenced it and restored the prior level. Switching
to AAC-ELD at 256 kbps preserved the endpoint, all three default render roles,
the volume setting and perceived loudness in both ears. Idle, reconnect and
other models still need volume checks.

With the 0.2.30 app and unchanged 0.2.29 native playback drivers, Pro 3 AAC
playback at 256 kbps passed two Conversation Awareness listening checks: speech
lowered the music and silence restored it; a volume chosen with NTPods' slider
during speech remained after speaking stopped. Commands targeting another output
and failed volume writes have regression-test coverage only. Other models remain
untested for this behavior.

On Pro 3, first microphone PCM arrived after 363 ms with AAC and 338 ms with
AAC-ELD. Recovery after stopping took 1.76 and 1.84 seconds respectively, with
zero malformed microphone packets. These short checks do not establish long-run
speech quality, clock drift or repeated interruption recovery.

The installed 0.2.48 app, preserving the existing native drivers, passed Pro 3
AAC-ELD checks at fixed 256 kbps, one frame per packet and a 20 ms source reserve.
The listener confirmed the startup hitch was gone through two pause/resume cycles.
A 20-second high-quality microphone check stayed clean in both ears during
capture and after stopping, with unchanged loudness and Windows volume. First
PCM arrived 376 ms after Start; capture had one initial discontinuity and none
afterward. The microphone returned to Idle 1.64 seconds after capture ended;
this is a lifecycle timing, not the duration of audible interruption. AAC with
the mic, long sessions and other models were not retested on this build.
The [1 kHz RTP comparison](WINDOWS-AUDIO-LATENCY.md#packet-and-source-reserve-overrides) failed;
restoring the default 48 kHz timestamps restored clean playback.

A 0.2.50 recovery preview passed two forced 250 ms backend stalls on Pro 3
A3064 / 9A348 with AAC-ELD at 256 kbps. Both caused PCM overruns and recovered
automatically; the listener confirmed clean stereo after each. The output's
identity, default roles and 53% volume remained unchanged. This checks repeated
overrun recovery, not sustained overload, deadline misses or other models.

Release 0.2.18 (`55601cb`) was also checked on a laptop running Windows 11
Enterprise build 26200, Intel Bluetooth driver 23.60.0.1 and Pro 3 A3064 firmware
9A348. Both codecs passed eight-second stereo checks at 256 kbps and 80% volume.
The paired output's identity and all three default render roles survived codec
changes, microphone tests and app/backend restarts. During and after a ten-second
capture, AAC stayed clear; AAC-ELD had a brief gap, then recovered in both ears.

First microphone PCM arrived at 482 ms with AAC and 536 ms with AAC-ELD;
capture-inactive to idle took 1.66 and 1.83 seconds. Each capture had one
discontinuity, with no nonfinite samples or timestamp errors. These are
microphone startup and shutdown timings, not acoustic playback latency. The
laptop check predates the experimental earbud-buffer trial.

Both test pairs advertise AAC-ELD. AAC fallback on a pair that lacks it has
software coverage only. Initial connection retries have automated cancellation
and retry-limit checks; the recorded Pro 1 rejection was recovered manually
before those retries were added.

Use the [playback test procedure](../windows/playback/TESTING.md) for other
models and adapters.

## Microphone checks with Alternative A2DP

Earlier microphone checks used Alternative A2DP 1.8.0.1 with AAC stereo at
48 kHz / 256 kbps. They are useful microphone evidence, but do not validate the
current native playback recovery path.

| Version | Result | Limit |
| --- | --- | --- |
| 0.1.7 | Twenty consecutive five-second captures produced PCM and returned to idle; both channels confirmed after the final cycle | Recovery interrupted playback for about 17.5 seconds; intermediate cycles had no acoustic check |
| 0.1.8 | Native 30-minute capture: 86,400,000 frames, one startup discontinuity, none afterward, no non-finite samples; automatic cleanup completed | No listener checked stereo after this sustained stop; no calibrated drift measurement |
| 0.1.8 | A second 20-second capture client closed without interrupting the main recording; disabling the feature silenced an open input | Full Discord and repeated multi-app sessions remain untested |
| 0.1.9 | Speech returned without static after both buds were removed and reinserted during one recording | Single-bud handoff and repeated interruptions remain untested |
| 0.1.18 | Manual Start/Stop, Keep active and Automatic with a five-second idle delay passed short live checks | Later driver changes need the same lifecycle checks |

A previous sustained capture ended with one-sided playback even though the
microphone uplink had stopped. That failure is why stopping the microphone still
runs playback recovery. A quiet uplink alone is insufficient to verify recovery.

## Controls, batteries and sensors

| Feature | Recorded result | Remaining limit |
| --- | --- | --- |
| Pro 1 listening modes and automatic pause | Off, Transparency and ANC worked; removal paused playback, reinsertion resumed it, and manually paused playback stayed paused (0.1.24) | Other models need their own check |
| Pro 3 native EQ | Custom values and Recommended mode returned the requested values after a fresh control connection (0.1.8) | Audible EQ effect not checked |
| Pro 3 stored settings | Press speed, hold duration, one-bud ANC, tone volume, call-gesture preference, volume swipe, personalized volume, Conversation Awareness, sleep detection and optimized charging passed change/readback/restore checks (0.1.8) | Stored values do not verify acoustic effects or physical gestures |
| Pro 3 Adaptive strength | Moving between the slider ends audibly changed outside sound (0.1.11); later continuous dragging was confirmed smooth in the installed app | Readback was coarse and absent immediately after writes; audible resolution was not measured |
| Pro 3 Allow Off | Off selection worked, and saved enabled/disabled choices survived reconnects (0.1.15) | Firmware did not acknowledge the Allow Off write itself |
| Quiet control connection | Mode changes remained usable after a two-minute idle check (0.1.25) | Longer idle and sleep/resume checks remain open |
| Closed-case batteries | Identity-matched BLE reports arrived after audio disconnected and the lid closed, with gaps near 60 seconds (0.1.20 and 0.2.18) | Reports eventually stopped; sleep duration and long-term charging accuracy are unknown |
| BLE scanning during playback | One-minute scan received 55 matching battery reports; listener heard no stutters (0.1.20) | Other Bluetooth devices were disconnected; microphone coexistence was not tested |
| Case charger while asleep | A 60-second scan found no paired report after connecting the charger with the lid closed (0.1.20) | Charger insertion cannot be relied on to refresh the case reading |
| Case cache | Known percentage remained after removing a bud; age stayed unchanged and charging cleared (0.1.21) | Cache is limited to the daemon session |
| Switching Pro 1 / Pro 3 | Correct identity, capabilities and battery values followed the connected pair (0.1.24) | Last-used-pair startup with both cases closed still needs a hardware check |
| Pro 3 head tracking | Turns and nods moved the graph; Pause and client exit stopped published samples (0.1.21). The later descriptor-based path delivered 311 updates over 9.813 seconds (0.2.21 preview) | Raw movement, not calibrated angles; prolonged microphone coexistence untested |
| Pro 1 head tracking | No accepted movement samples on firmware 6F21 (0.1.24) | Compatibility unresolved |
| Custom Transparency | Pro 3 / 9A348 with AX200 and Apple host identity: reads, enable, an audible noise-reduction adjustment and restoration passed (0.2.25 preview with the unenrolled-status fix); installed 0.2.26 read the restored values | Other models, reconnect persistence, extended use and nonzero EQ preservation remain unverified; Windows SDP response not captured |
| Hearing assistance | Development preview, Pro 3 / 9A348, AX200 and Apple host identity: read an iPhone-fitted profile; confirmed independent Hearing Aid and Media Assist changes through a control refresh; listener reported changes to surrounding sound and music; fitting bytes stayed unchanged | The fitted layout remains read-only; fitting writes, other models, long sessions and reconnect persistence remain unverified |
| Loud Sound Reduction | Pro 3 A3064 / 9A348 returned enabled on Windows with 0.2.25, before the identity override | No toggle or acoustic check |

The 0.2.18 battery check initially found scanning disabled in the running process
despite an enabled saved preference. Reapplying it restored scanning and it
survived an app restart; the original mismatch was not explained.
Heart-rate accuracy, multipoint behavior and newer-model sensor compatibility
also remain unverified. See [feature behavior](WINDOWS-FEATURES.md) and the
[protocol reference](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md).

## Playback latency investigation

A wired microphone through a Roland Rubix 24 recorded chirps beside one Pro 3
bud. QPC timestamps aligned Windows loopback with acoustic capture. Measurements
include uncalibrated wired-input delay; they are not Bluetooth transport latency.

With installed 0.2.17, both codecs at 256 kbps and a 50 ms source reserve:

| Codec | First run median | Second run median |
| --- | --- | --- |
| AAC | 273.3 ms | 280.4 ms |
| AAC-ELD | 241.0 ms | 243.8 ms |

Each accepted run matched all six chirps, with correlation at least 0.873 and no
timestamp errors or discontinuities after startup. Playback used a 480-frame
(10 ms) shared engine period. One run with a missing acoustic marker was excluded.

Earlier 30 ms reserve trials measured AAC at 254 ms and AAC-ELD at 241–244 ms,
compared with 268 ms and 247 ms in the initial 50 ms runs. The repeated 50 ms
comparisons were contaminated by other audio and excluded. This incomplete A/B
test does not establish a latency benefit from the smaller reserve. Volume and
startup variation also prevent treating small differences between sessions as a
code improvement.

With the 0.2.31 app and installed 0.2.29 native drivers, the shared render
endpoint offered a 240-frame minimum and 480-frame default at 48 kHz. Both
default and RAW mode accepted 240-frame (5 ms) initialization with the mix format
and PCM16. The active engine period became 240 frames while each client existed
and returned to 480 after release. Those checks did not start audio. Earlier
0.2.18 checks had exposed only 480 frames for shared render; the intervening
cause was not isolated. The earlier capture and exclusive render checks also
accepted 240 frames. A subsequent 30-second silent shared client held the
engine at 240 frames while AAC-ELD played at 256 kbps; the listener confirmed
clean stereo. Start/end and an independent query during the hold reported
240 frames. Immediately after release it remained at 240, then a later query
reported 480. Volume and output selection stayed unchanged. Loaded-system,
long-session and acoustic latency checks remain open. See
[audio latency](WINDOWS-AUDIO-LATENCY.md) for the reproducible shared-mode probe.

The 0.2.32 preview with unchanged 0.2.29 native drivers passed a short listening
check on Pro 3 A3064 / 9A348 and AX200 using AAC-ELD Variable mode at 256 kbps
peak, one frame per packet and a 30 ms source reserve together. Music stayed
clean in both ears through two pause/resume cycles, a 20-second high-quality
microphone capture and recovery after stopping. First microphone PCM arrived
after 368 ms, capture had one startup discontinuity and none afterward, and
stop recovery took 1.82 seconds. The installed 0.2.33 app, with unchanged drivers
and the same trial settings, also passed music and two pause/resume cycles.
Volume stayed unchanged. These checks used a 30 ms reserve.

Installed 0.2.35 with its updated native drivers passed short Pro 3 A3063 / 9A348
checks on AX200: AAC-ELD Variable at 256 kbps peak with one frame per packet and
a 20 ms reserve, and AAC at 256 kbps with a 30 ms reserve. Both kept music clean
through pause/resume and codec changes, with the selected volume preserved.
AAC-ELD also stayed clean during a 20-second high-quality microphone capture and
after stopping. First microphone PCM arrived after 477 ms; capture had one
startup discontinuity, no malformed packets and no nonfinite samples. Stop
recovery finished in 1.55 seconds.

Read-only endpoint queries reported an active shared period of 240 frames at
48 kHz (5 ms) during playback with each codec and after microphone recovery.
The probe did not initialize an audio client or request that period. These
checks validate short playback compatibility, not acoustic latency or the
driver's notification completion path. Long sessions, loaded-system behavior,
disconnects, sleep/removal and other models remain untested for this combination.

### Earbud buffer control

With NTPods 0.2.26, Pro 3 A3064 / 9A348 and AX200, 30-second native-playback
trials at 256 kbps kept music clean in both ears during the request and restore:

| Codec | Requested target | Supplied restore target | Report after request | Report during restore |
| --- | --- | --- | --- | --- |
| AAC | 100 ms | 150 ms | 149 ms | 144 ms |
| AAC-ELD | 60 ms | 150 ms | 150 ms | 142 ms |

Each report arrived within the command's settling interval. These values do
not establish that the earbuds reached either target. The restore send completed
without a backend error, but no remote buffer-setting readback exists. AAC-ELD
also reported zero before its media stream started; that is not measured latency.
The trials kept the 50 ms source reserve and 200 ms media flush timeout unchanged.

A continuous 60-second AAC-ELD recording compared the same stream before,
during and after a 60 ms request, with 150 ms supplied for restoration:

| Phase | Median acoustic delay | Accepted markers |
| --- | --- | --- |
| Before request | 253.4 ms | 3 |
| 60 ms requested | 252.5 ms | 8 |
| After 150 ms restore send | 245.9 ms | 5 |

The Roland Rubix 24 captured one bud beside the wired microphone at 100% output.
A centered 700–6500 Hz FIR filter attenuated low-frequency noise without adding
delay. Matching required the full five-chirp identity; nine weak markers were
rejected out of 30, including four consecutive late-trial markers. Phase
boundaries were trimmed by two seconds. Neither stream
reported timestamp errors or discontinuities after startup. Packet timestamps
still disagreed with sample-count extrapolation by up to about 4 ms; this is not
a calibrated measurement-error bound. Wired-input delay remains uncalibrated.

The 0.9 ms trial difference does not demonstrate an improvement. Delay was
already declining before restoration; rejected markers and timestamp variation
limit comparison of the phase medians. This single run cannot exclude a small
or delayed effect. Exact remote buffer restoration also remains unverified. See the
[coordinated macOS transition](MACOS-AUDIO-CAPTURE-2026-10-12.md#coordinated-audio-adjustment)
for the additional host audio adjustment absent from this trial.

With 0.2.28 on the same Pro 3/AX200 setup, an AAC 256 kbps trial requested
120 ms with a 150 ms restore target. Music remained clean in both ears. A fresh
streaming delay report fell from 162 to 129 ms after a finite 32 ms host-clock
adjustment. The normal clock and 200 ms media flush timeout were restored,
and playback restarted successfully. No fresh streaming report verified the
final receiver buffer; this was a listening check, not an acoustic latency
measurement.

With 0.2.29, the matching 30-second AAC-ELD trial at 256 kbps kept music clean
in both ears with steady loudness, apart from the expected brief pauses at
stream restart. The stream reported 149 ms after the 120 ms request; a finite
19 ms host-clock adjustment followed. A 129 ms report arrived immediately after
the 150 ms restore request, before its 21 ms reverse clock adjustment. The normal
clock and 200 ms media flush timeout were restored, and playback resumed without
reported errors. Neither report verifies a requested buffer target or measures
acoustic latency; exact remote-buffer restoration remains unverified.

The 0.2.35 preview with the zero-report resume fix passed two automatic
restore/reapply cycles on Pro 3 A3063 / 9A348 and AX200, using AAC-ELD Variable
at 256 kbps peak and a 20 ms source reserve. Each paused configuration reported
zero; after resume, the 120 ms request received a fresh 137 ms report before a
finite 7 ms clock adjustment. The listener confirmed clean stereo, unchanged
loudness and working pause/resume. Zero was not used as a clock input.

Automatic microphone demand restored the 150 ms target, normal clock and
200 ms flush timeout before starting capture. Recovery after stopping took
1.84 seconds, with no malformed packets. The listener confirmed clean stereo
and unchanged loudness during capture and after recovery. The restored
configuration reported 102 ms, so automation left it unchanged. These reports
do not verify the remote buffer setting or establish acoustic latency.
Sustained operation, loaded-system behavior and other models remain untested
for this policy.

## Software and interface checks

[`windows/build.ps1`](../windows/build.ps1) runs the automated checks described
in [BUILD-WINDOWS.md](BUILD-WINDOWS.md) and [windows/tests](../windows/tests/README.md).
They cover protocol validation, IPC, microphone lifecycle, PCM bounds, encoder
output, driver builds and installer rollback logic. Encoder round trips check
pitch and channel separation. AAC-ELD Variable mode has offline tone, silence,
noise and transient coverage, plus the short Pro 3 listening check above.

Native UI checks have covered light/dark themes, narrow and wide windows, device
changes, cached/missing batteries, submenu motion, keyboard focus and localized
labels. Automated motion checks cover interruption and reduced motion. These
checks do not guarantee frame rate on another PC or replace assistive-technology
testing. The Windows reduced-motion setting and high-DPI hardware still need
manual coverage.

Installation and upgrades have succeeded on the test PC, including package/file
comparison and driver startup. Failure and rollback tests mock privileged Windows
operations. A real failed upgrade, complete driver removal and restoration on
other PCs remain unverified.

## Remaining acceptance checks

Before treating a new build as hardware-tested, record its version or commit,
Windows and Bluetooth driver versions, AirPods model/firmware, selected codecs,
bitrates and application input/output choices. Keep recordings and personal
identifiers out of the repository.

- Repeat sustained stereo playback and microphone capture with NTPods' current
  playback driver, including speech pitch, clock drift and post-stop stereo.
- Repeat microphone start/stop, overlapping clients, ear removal, case closure,
  Bluetooth toggle, sleep/resume, app exit and backend crash. Check for silence
  while disabled, stale PCM, missing endpoints and one-sided playback.
- Exercise failed recovery, retry and install/upgrade/uninstall on a prepared test
  PC. Verify restoration for both connected and temporarily absent pairs.
- Check access from another ordinary Windows account, then run Driver Verifier
  against the three NTPods drivers during I/O, removal and suspend/resume.
- Test unverified models, AAC-only fallback, sustained AAC-ELD Variable playback,
  Custom Transparency, hearing controls and sensor coexistence separately.

## Production release requirements

The ACX sample licensing discrepancy remains unresolved; see
[THIRD_PARTY.md](../THIRD_PARTY.md). Author clarification or a compatible
replacement is required. Production deployment also needs Microsoft-signed
packages that load with Secure Boot and Memory Integrity enabled. Local test
signing does not establish that compatibility.
