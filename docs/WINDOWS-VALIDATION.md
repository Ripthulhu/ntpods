# Windows implementation and acceptance

Target: Windows 11 x64, Intel AX200, AirPods Pro 3 firmware 9A348. Source baseline:
`188e45df20259b616ce155be812086259d1a2488`. This build is experimental.

References to `../../outputs` identify local test logs and captures. Those files
are not included in the repository; tests needed to repeat the automated checks
are under `windows/tests`, `windows/daemon/tests` and the driver test folders.

Playback constraint: preserve the installed Luculent Systems Alternative A2DP
Driver 1.8.0.1 (`oem21.inf` on this PC). The user reports AAC at 48 kHz / 256 kbps.
Read-only inspection confirms this driver owns the AirPods AudioSink service
`0000110b-0000-1000-8000-00805f9b34fb`; NTPods binds the separate Apple control
service `74ec2172-0bad-4d01-8f77-997b2be0722a`. Neither NTPods package replaces the
A2DP playback driver. Audio recovery reopens the service, so negotiated codec,
bitrate and channel separation must be rechecked afterward. Codec configuration
remains in Alternative A2DP Driver, not NTPods.

## Initial implementation (through 0.1.8)

- IPC permits the running user and SYSTEM, rejects remote clients, bounds command
  size and event queues, and checks server ownership in the WinUI client. Driver
  injection/control permits the account installed by the MSI and SYSTEM. Invalid
  or missing driver identity policy fails closed to SYSTEM.
- The raw Bluetooth proxy is removed from the normal daemon. Microphone messages
  never enter UI events or general packet logging. Timing/count diagnostics contain
  no microphone payload or PCM.
- PCM writes are bounded and aligned. Closing the producer, ending capture or
  disabling the feature clears queued audio. The virtual input remains present.
- Microphone use is opt-in (initially off), with its preference saved for the next
  launch. A running Windows capture stream is also required. Manual commands from
  older clients cannot leave the physical microphone armed while unused.
- IPC and UI distinguish capture clients from the Bluetooth uplink, with idle,
  starting, capturing, stopping, recovering, unavailable and error states.
- Stop closes the PCM gate immediately and sends STOP_AUDIO. A quiet uplink did not
  prevent one-sided playback after the sustained 0.1.6 capture. Version 0.1.7 also
  reopens the target AirPods' audio services after each attempted microphone stream,
  using the same path that restored both earbuds during the manual check. The
  microphone remains silent in `recovering`; a new capture waits for restoration
  and a 500 ms quiet window. Completed cleanup does not send another STOP.
  If packets continue, one STOP retry follows a two-second drain window, with one
  further bounded drain. Recovery permits one service reset and one AirPods-only
  fallback reconnect; an unsuccessful attempt disables the microphone and reports
  an error. Each service operation queries fresh
  device information on the selected radio, verifies the exact remembered and
  authenticated target, and never falls back to an unspecified radio. Automatic
  service rebuilds after ordinary reconnect or keepalive failure have been removed.
  The tested KS one-shot recovery was unsuccessful and is excluded from this build.
  Ordinary automatic stops passed on this PC; broader recovery cases remain open.
  Graceful application exit also runs the stop/recovery sequence. Recovery attempts
  are bounded; Windows' synchronous Bluetooth service API itself has no cancellation
  timeout. Graceful shutdown allows 45 seconds for cleanup and waits up to 60
  seconds before closing process handles. Unfinished cleanup remains an error,
  not successful restoration.
- Noise and feature state come from device replies. Reconnecting requests fresh
  notifications and clears cached battery/ANC state. Ear insertion resumes only the
  exact media session whose pause request this daemon previously completed.
- Conversation Awareness requests remain visibly pending until a device reply
  confirms the setting. Sending a command alone does not change confirmed state;
  the application does not reconnect automatically to obtain acknowledgment.
- The decoder remains AAC-ELD in user mode and exports 48 kHz mono PCM. The inherited
  64 kHz source-clock assumption and gain remain unverified for 9A348. The user
  confirmed normal pitch/speed on a short Sound Recorder playback; calibrated pitch,
  end-to-end latency and long-term clock drift remain unmeasured.
- Capture suppresses host Conversation Awareness ducking and restores the current
  ducked volume. It does not temporarily alter the AirPods' stored CA preference.
- Version 0.1.8 adds native EQ and the device controls listed in
  [WINDOWS-FEATURES.md](WINDOWS-FEATURES.md). Unknown settings remain unavailable;
  requests remain separate from confirmed device state. Idle-only settings refresh
  checks live capture status before reopening the control connection.
- Start with Windows is enabled on the development PC. The updated installer
  preserves the existing startup choice through upgrades, and the UI checks both
  app and daemon entries. Opening the 0.1.8 UI reuses the running daemon; a second
  UI launch activates the existing window instead of interrupting microphone use.

## Software checks

Run `windows/build.ps1` with the pinned tools from [BUILD-WINDOWS.md](BUILD-WINDOWS.md).
The Rust tests exercise malformed microphone records, lifecycle cancellation and
bounded recovery, real Windows pipe ACL construction, and actual FFmpeg ELD loading
and invalid-buffer handling. Cross-language IPC checks use the production Rust and
C# models. The portable driver tests exercise production validation/ring code,
including malformed IOCTL lengths, SID injection and randomized ring operations.

These checks do not load a kernel driver or establish that audio works on hardware.
The 0.1.7 candidate passed all 26 daemon tests, IPC checks, portable driver tests,
WDK builds, and installer checks under PowerShell 7 and Windows PowerShell 5.
Build artifacts and hashes are recorded in `windows/build-output/build-manifest.json`
after a successful build. Windows driver/header compilation and portable sanitizer
tests are separate from Driver Verifier on a loaded driver.

## Hardware acceptance record

Partial results recorded **2026-10-11**, using MSI **0.1.1** and separately tested
daemon runtimes **0.1.2 / 0.1.3 / 0.1.4 / 0.1.6-final / 0.1.7-final** on its installed drivers, on Windows 11 Enterprise
multi-session build **26200**, Intel Bluetooth driver **24.10.0.4**, AX200 and AirPods
Pro 3 firmware **9A348**. These are initial hardware checks, not completed acceptance.

- Clean installation and the 0.1.0-to-0.1.1 MSI upgrade succeeded. Both NTPods
  drivers reported Running with PnP problem code 0. The ordinary installed user
  successfully opened IPC and both driver control devices.
- Alternative A2DP remained Luculent **1.8.0.1 / oem21.inf**. Its status UI still
  showed **AAC, stereo, 48 kHz, 256 kbps** after the test.
- Noise-control device replies confirmed modes **3, 4 and 2**. Both buds reported
  **89%** battery; case battery and repeated refresh/reconnect behavior remain pending.
- With the microphone feature disabled, a 10-second capture returned **480,000
  zero-valued PCM frames**. The capture-active flag cleared after the last client
  closed. A 20-second enabled capture returned **960,480 frames with real audio**;
  the user confirmed distinct left/right playback during capture.
- Microphone-stop acceptance **failed in 0.1.1**: the unconditional audio-service
  reset returned **0/87**, disrupted playback and led to an approximately **17-second
  reconnect**. Runtime 0.1.2's quiet-stop correction subsequently completed short
  stops in approximately **752 ms**, with the uplink off and no service reset.
- Windows Sound Recorder captured and played back **34 seconds of actual voice**.
  The user confirmed clear speech at normal pitch and speed. Alternative A2DP's UI
  showed **AAC stereo / 48 kHz / 256 kbps during capture**. This is a subjective
  short speech check, not a calibrated pitch/drift measurement or Discord validation.
- A **25-second alternating left/right signal**, including **10 seconds of capture**,
  stayed stereo through microphone stop according to the user. The capture returned
  **480,000 frames**, including **436,702 nonzero samples**. The user clarified that
  the final left-only beat was the expected final test tone, not a stuck playback channel.
- Two overlapping native WASAPI capture clients worked: closing the shorter client
  did not interrupt the remaining client's PCM. Disabling the feature at approximately
  **13 seconds** yielded zero PCM while the other client remained open until **20
  seconds**. State reached idle/uplink-off approximately **753 ms** after disable;
  the capture-active flag stayed true until that last client closed.
- The rapid **20-cycle test stopped at cycle 7** on runtime 0.1.3.
  Cycles **1–6** produced real PCM and reached idle/uplink-off. Cycle **7** delivered
  **96,000 zero-valued frames**, then kept receiving microphone packets after STOP:
  **58 packets by 2,118 ms**, rising to **224 by 7,103 ms**. The bounded recovery
  reset returned **0/87**, followed by the targeted fallback returning **1168/0**.
  The user heard brief individual-bud dropouts and then a full reconnect. The final
  state was error/feature-off/AAP-disconnected, requiring explicit retry; it did not
  enter an automatic retry loop. **Repeated capture/stop acceptance has failed and
  remains a release blocker.** The bounded STOP retry is under hardware retest.
- Runtime 0.1.4's next rapid-cycle attempt passed **10 cycles**, then cycle **11**
  returned **96,000 zero-valued frames**. That attempt ended at idle/uplink-off,
  rather than the continuing-uplink failure in 0.1.3, but still failed capture
  acceptance. Twenty consecutive successful cycles have not been demonstrated.
- A scoped KS recovery probe confirmed that the exact Luculent AudioSink supports
  the documented one-shot operations. Its first action test returned **0/1460**:
  disconnect succeeded but reconnection exceeded the initial two-second wait.
  The same endpoint was observed returning after approximately six seconds.
  A concurrently running older daemon then invoked its inherited automatic service
  rebuild and removed stereo playback. A second isolated test with no daemon
  running and an eight-second reconnect wait also returned **0/1460**, with the
  same endpoint remaining unplugged. **KS recovery failed** and was removed from
  production code. The next build retains bounded Bluetooth service recovery with
  fresh device information for each operation and removes the automatic rebuild
  triggers; neither change establishes recovery acceptance yet.
- The retained production service-recovery path also **failed** an isolated check
  after refreshing device information: result **0/87**, elapsed **4,400 ms**. A
  subsequent explicit enable operation returned 0 and restored stereo at 48 kHz
  with Luculent `oem21.inf` still installed. This separate repair does not turn the
  failed bounded recovery into a pass. **Deep recovery remains a release blocker.**
  Runtime **0.1.6-final**, executable SHA-256
  `C207145C1B15956A79658EA3D65FFBCB026E27875CEAEE5B9CE897AEAE5B10A6`,
  passed a live soft STOP retry before starting the longer capture test.
- The 0.1.6-final sustained recording ended early. The native capture
  recorded **42 complete intervals covering 1,260.3179 seconds (21:00.318)** and
  **60,494,880 frames**. It reported one startup discontinuity, zero nonfinite
  samples and zero empty intervals. The final partial interval was not counted.
  Sound Recorder saved **Recording 4**, displayed as **20:52**. The user reported
  a good experience while moving around during recording. The planned 30-minute
  duration was not completed.
- The daemon measured **1,283.258 seconds**, with first packet and PCM at **386 ms**,
  **42,761 packets**, **0 invalid packets**, **171,044 audio units**, and
  **61,575,824 frames decoded and delivered**. There was no retry, reset or
  reconnect during the run. These counters describe capture continuity; calibrated
  latency and clock drift remain unmeasured.
- After capture closed, STOP reached quiet in **753 ms**, with one late packet
  and no retry or reset. The user then heard audio from **only the right bud**.
  **Stereo playback after microphone stop failed**, despite the quiet uplink.
  Disabling the microphone and repairing the AAP connection did not restore both
  channels. Explicit `RebuildAudio` then disabled and enabled only the target
  AirPods audio services, without resetting the radio. It ran from
  **22:59:26.465Z to 22:59:41.951Z**, returned **0/0** after **15.486 seconds**, and
  the user confirmed both buds worked again. Alternative A2DP showed **AAC,
  stereo, 48 kHz, 256 kbps** after reconnect. This manual recovery succeeded for
  this event; automatic restoration after STOP still failed, and the earlier
  recovery errors remain unresolved. The microphone feature was left **off**.
- Runtime **0.1.7-final**, SHA-256
  `B8E5D7D8E1EB8CE86CA8F7D608E886D2FF7241A2F8A05486B58DC5EEECCD1BD1`,
  completed an eight-second capture with **384,000 frames** and **357,109 nonzero
  samples**. After STOP, no further microphone packets arrived. The automatic
  audio-service reset returned **0/0**, and cleanup reached idle in **16.970
  seconds** (observed by the harness at 17.535 seconds). AAP stayed connected and
  the virtual input remained available. The user confirmed separate left/right
  tones after recovery; Alternative A2DP reported **AAC stereo, 48 kHz, 256 kbps**.
  This passes one ordinary automatic stop, with an audible playback interruption.
- The same 0.1.7 runtime then passed **twenty consecutive five-second capture/stop
  cycles**. Every capture produced nonzero PCM; every stop returned to idle with
  no active capture or uplink and no error. Observed stop completion ranged from
  **17.451 to 17.536 seconds**, averaging **17.489 seconds**. Across the initial
  check and these cycles, all **21 service resets returned 0/0**; no STOP retry or
  fallback reconnect was needed. The user confirmed distinct left/right tones
  after cycle twenty, and Alternative A2DP still reported **AAC stereo, 48 kHz,
  256 kbps**. Channel separation was checked after the initial capture and final
  cycle, not acoustically after every intermediate cycle. Long-capture shutdown,
  forced recovery failures and device interruptions still need retesting.
- MSI **0.1.7** upgraded the installed 0.1.1 build successfully, returning **3010**
  (restart required). Windows was not restarted. Both drivers nevertheless reported
  Running and PnP status OK, and the installed app launched the exact daemon tested
  above. A five-second capture from the re-created virtual endpoint produced
  **240,000 frames / 203,807 nonzero samples** and completed automatic stop recovery
  in **17.527 seconds**. The UI showed silent microphone input during recovery.
  Alternative A2DP remained version **1.8.0.1**, with **AAC stereo, 48 kHz, 256 kbps**.
  The user confirmed separate left/right tones after this installed-app stop too.
  Automatic microphone use was left enabled with no capture client or active uplink.
- Windows restarted at **2026-10-11 01:30 Europe/Amsterdam**. Both
  NTPods drivers were Running afterward; the installed daemon still matched the
  tested executable hash. NTPods was opened manually after login and retained its
  automatic microphone preference. A five-second capture produced **239,520 frames
  / 207,310 nonzero samples**. Stop recovery returned **0/0** and reached idle in
  **17.544 seconds** as observed by the harness, with capture and uplink inactive.
  Alternative A2DP still reported **AAC stereo, 48 kHz, 256 kbps**. This boot check
  did not repeat the acoustic channel-separation test or the sustained recording.
- Conversation Awareness writes **apply and are confirmed on a fresh AAP connection**:
  setting on and reopening AAP produced device value **1 / true**; setting off and
  reopening produced **2 / false**, restoring off afterward. The reconnect used the
  AAP repair path with audio enable only. Live notification requests still did not
  echo the changed setting, so immediate UI acknowledgment remains unresolved.
  Acoustic CA behavior and ear pause/resume remain untested.

### Version 0.1.8 feature checks

The packaged daemon with SHA-256
`0A15EA9F4C6DB8E5840B8A50DEC0097F0E6A96997A835136F464E2E88DD91C22`
and managed UI DLL with SHA-256
`A000DF03043C46E26DCD8B215B718592FF71B974DAD5E9BE922E5B3E1F76F78E`
ran against the installed drivers. Native EQ and ten device-control
change/readback/restore checks passed; details and unverified controls are in
[WINDOWS-FEATURES.md](WINDOWS-FEATURES.md). Settings generally did not echo their
new values immediately; reopening AAP supplied confirmed values. No driver or
A2DP codec setting was changed for these tests.

The earlier 0.1.7 overnight capture was interrupted at **1,190.151 seconds** when
the app-launch tool selected the installed UI instead of the requested test UI.
That older UI shut down its daemon. The 39 complete intervals before the
interruption reached **1,170.287 seconds**, with one startup discontinuity and
none afterward. Automatic shutdown recovery returned **0/0** and confirmed a
quiet uplink after **16.614 seconds**. This is an incomplete run, not a pass of the
30-minute requirement. A separate 20-second overlapping capture client closed
earlier without stopping the main capture. No microphone samples were saved.

The new UI rendered without clipping at 986- and 671-pixel window widths. An
unsent EQ draft survived a backend state refresh. Launching the new UI and then a
second instance preserved the active daemon and capture. Screen-reader Apply
names now identify their settings. Start with Windows was verified in the
installed app and both Run entries; next-login behavior has not been repeated.

The same 0.1.8 daemon completed a native **30-minute capture** from
**2026-10-11 02:14:29 to 02:44:29 Europe/Amsterdam**: **1,800.0083 seconds**,
**86,400,000 frames**, **84,338,560 nonzero samples**, one startup discontinuity,
zero later discontinuities and zero nonfinite samples. All 59 complete interval
rows contained audio; the final total includes the remaining partial interval.
The daemon received **59,985 packets with zero invalid packets** and delivered
**86,378,384 decoded frames**. First packet and PCM arrived at 380 ms. These are
continuity measurements, not calibrated latency or clock-drift measurements.
No microphone samples were saved.

A second native capture overlapped for **20.004 seconds / 960,000 frames**.
Closing it left the main capture and uplink active without STOP or recovery.
After the last client closed, STOP reached quiet at **753 ms**, with six trailing
packets. The automatic audio-service reset returned **0/0** and final cleanup
reached quiet/idle at **16.899 seconds**, without STOP retry or reconnect fallback.
The final state was connected, automatic microphone enabled, capture and uplink
inactive, and no error. Alternative A2DP reported **AAC stereo, 48 kHz, 256 kbps**
during capture. This passes native capture continuity and automatic cleanup;
it does not establish acoustic stereo after this stop, a Sound Recorder or
Discord duration test, or calibrated drift.

The final 0.1.8 package contains daemon SHA-256
`0B6C90C6CED6FA7EC2BAF491042EF5874AD7749BB29E4BFAD9ED41B938A667C8`
and managed UI DLL SHA-256
`AE9188EA1F22E3240BADC80280649F16F3EBE56FD1419353B7D804EA55850E50`.
This revision adds guarded settings refresh and keeps Adaptive strength disabled
while an ANC change remains unconfirmed. Its checks passed: **41 daemon tests,
3 IPC tests, 8 state and 6 command cross-language fixtures, 31 malformed C# input
cases**, native driver/header checks, WDK builds, startup tests and the
**40-case MSI startup matrix under PowerShell 7 and Windows PowerShell 5**.
The existing `IslandView` CS8604 warning remains. Packaged files and the pinned
source snapshot matched their manifest; FFmpeg imports and static CRT were checked.

The final daemon then passed idle refresh and two ten-second active-capture
refresh tests. With the microphone feature off, the client received **480,480
silent frames**; with it on, **480,480 frames / 441,668 nonzero samples**. Both
refresh requests were rejected without disconnecting or stopping capture early.
The disabled-feature request did not run later when the client closed. Each
case ended connected and idle without a microphone error. Clicking the final
WinUI refresh button while idle supplied the original EQ and device settings,
cleared the earlier rejection warning, disabled the button during its cooldown
and then enabled it again. Automatic pause and microphone use remained enabled.
Alternative A2DP again reported **AAC stereo, 48 kHz, 256 kbps** after recovery.

MSI 0.1.8 SHA-256 is
`58CC3561DC4B77856F658C619625952FDAC39B11E3204EF1BA4B2EACBC9A199F`.
It was **not installed** at the end of the overnight tests; administrator
approval was still pending. Both driver binaries were
unchanged. All native capture clients and portable test processes were stopped;
the installed 0.1.7 app was restored, connected and idle, with automatic microphone
use on and both Start with Windows entries still pointing into Program Files.
The 30-minute test used the earlier 0.1.8 hash above; the final revision received
the shorter refresh/capture regression, not another duration run.

### Daytime installation and repair

The upgrade from 0.1.7 on **2026-10-11** returned
**3010**, requesting a restart, but left the WinUI EXE and DLL missing. The old
files had PE file version **1.0.0.0**; the 0.1.8 build lowered it to **0.1.8.0**
while keeping the same MSI component GUIDs. Windows Installer rejected those
components during costing because higher-version files existed, then removed
the old files during `RemoveExistingProducts`.

A repair of the same verified MSI with `REINSTALL=Main REINSTALLMODE=p` returned
**0** at **09:15:44 UTC** and restored the missing files. The repair log confirms
`drivers: False`, no driver installation/removal and no scheduled restart.
All **565 installed WinUI files** matched the final distribution; the installed
daemon, UI EXE and managed DLL had the expected hashes. Future packages need
monotonically increasing PE file versions and a build check against the previous
MSI's File table. The display version can be set separately. The tested package
was repaired without changing its bytes; the original upgrade's restart remains
pending.

The installed app reports **v0.1.8** and runs from Program Files. Both NTPods
drivers are running and both devices have problem code **0**. The AirPods
reconnected, reported both buds and case batteries, and returned the original
controls and Recommended EQ at **50/50/50**. The virtual **48 kHz mono** input is
available; automatic microphone use is on and the state is connected and idle
without an error. Both buds were charging in the case, so no new capture or
listening check was attempted after installation. The installer logged the
intended user SID; direct reads of the protected driver policy keys were denied
to the unelevated process.

Start with Windows is on in the installed UI and both Run entries point to
Program Files. However, the real upgrade and repair logs contain no populated
`STARTUPAPP`/`STARTUPDAEMON` search results and pass `StartupMode off` to setup.
The entries remained present. Their current state is verified, but this does
not validate the intended MSI startup snapshot path. Investigate that discrepancy
with a real-account upgrade test; the mocked startup matrix did not expose it.

Local feature evidence is retained under `../../outputs/`:
`ntpods-018-controls.jsonl`, `ntpods-018-eq-custom-live.jsonl`,
`ntpods-018-eq-custom-readback.jsonl`, `ntpods-018-eq-restored.jsonl`,
`ntpods-018-adaptive-roundtrip.jsonl`, `ntpods-018-ui-review.json`,
`ntpods-018-ui-capture-launch.json`, `ntpods-018-ui-single-instance.json`,
`ntpods-017-overnight-incomplete.json`, `ntpods-017-interrupted-stop-log.txt`,
`ntpods-017-overnight-second-client.txt`, `ntpods-018-thirty-minute.result.json`,
`ntpods-018-thirty-minute.capture.txt`, `ntpods-018-thirty-minute.daemon-summary.txt`
and `ntpods-018-thirty-minute-a2dp.json`. Final-package evidence is in
`ntpods-018-artifact-verification.json`, `ntpods-018-final-refresh.jsonl`,
`ntpods-018-final-state.jsonl`, `ntpods-018-final-ui-and-a2dp.json` and
`ntpods-018-restored-installed-app.json`. Daytime installation evidence is in
`ntpods-upgrade-018.log`, `ntpods-upgrade-018-result.json`,
`ntpods-upgrade-018-repair.log`, `ntpods-upgrade-018-repair-result.json` and
`ntpods-upgrade-018-final.json`.

Earlier local evidence is retained under `../../outputs/`: `ntpods-cycle-results.jsonl`,
`ntpods-014-cycle-results.jsonl`, `ntpods-ks-btaudio-support.txt`,
`ntpods-ks-recovery-hardware.txt`, `ntpods-ks-recovery-isolated.txt`,
`ntpods-service-recovery-fresh-info.txt`,
`ntpods-016-long-capture.txt`, `ntpods-016-long-observations.jsonl`,
`ntpods-016-long-run.json`, `ntpods-016-long-states.jsonl`,
`ntpods-017-first-acoustic-result.json`, `ntpods-017-twenty-cycles.jsonl`,
`ntpods-017-cycle-summary.json`, `ntpods-017-mic-events.txt`,
`ntpods-017-installed-cycle.jsonl`, `ntpods-upgrade-017-result.json`,
`ntpods-017-postboot-cycle.jsonl`,
`ntpods-012-stereo-retest.txt`, `ntpods-012-stereo-mic-measurements.txt`,
`ntpods-multiple-disable-client-a.txt`, `ntpods-multiple-disable-client-b.txt`,
`ntpods-multiple-disable-states.txt`, `ntpods-ca-fresh-connection-on.jsonl` and
`ntpods-ca-fresh-connection-off.jsonl`. The latter records confirm CA write/readback
on reconnect; the earlier Conversation Awareness control records capture the missing live acknowledgment.
Stop timings and recovery codes are in `%LOCALAPPDATA%\NTPods\daemon.log`; its UTC
22:14 cycle entries correspond to local **2026-10-11 00:14**. User listening reports
supplement these measurements; they do not establish long-run stability.

| Area | Status | Procedure and pass condition |
| --- | --- | --- |
| Install | 0.1.8 installed after a targeted repair; restart completed, both drivers Running with problem code 0 | Keep PE versions increasing and check installed files against the package. Post-reboot capture has not been repeated. Keep the MSI for removal. |
| Alternative A2DP | Preserved; AAC stereo 48 kHz / 256 kbps confirmed during 0.1.8 sustained capture and after final-build recovery | Recheck driver, codec and stereo after upgrades and uninstall. SBC/HFP fallback fails. |
| Controls | Native EQ and ten stored-setting roundtrips passed on 0.1.8; live acknowledgment often absent | Complete physical gestures, acoustic effects, Adaptive while worn, unreported settings and repeated reconnects; require confirmed device state without duplicate sessions or stale values. |
| Ear detection | Pending | Remove/reinsert each bud while playing, already paused and after changing media applications. Resume only playback NTPods paused. |
| Stereo coexistence | 0.1.7 restored both channels automatically after short capture and twenty cycles; 0.1.8 sustained cleanup passed without an acoustic listener | Check separate-ear playback during and after sustained capture. Prior sustained 0.1.6 stop failed. HFP fallback and duplicated mono fail. |
| Clock/pitch | Short speech sounded normal; calibrated checks pending | Compare acoustic reference pitch, recording duration and start/end latency; inspect PCM rate/gap counters. No growing delay. |
| Sustained recording | 0.1.8 native 30-minute capture and automatic stop passed; earlier Sound Recorder sample lasted 20:52 | Complete 30-minute Sound Recorder and Discord runs with acoustic stereo checks and restored channels after stop; measure drift and latency. |
| Twenty cycles | Passed twenty five-second cycles on 0.1.7; acoustic stereo confirmed after the last | Extend coverage to rapid reopen during recovery and longer captures. Earlier 0.1.3/0.1.4 failures remain recorded above. |
| Multiple clients | Initial overlap/disable checks and a 20-second overlapping client during the 0.1.8 long capture passed | Extend overlap and feature off/on coverage; audio continues only while needed and stops after the last client. |
| Disable/crash | Silence while capture remained open passed; crash testing pending | Measure disable latency precisely, then test crash recovery and absence of replayed old audio or unsolicited uplink. |
| Interruptions | Pending | Test case closure, Bluetooth toggle, removal, application exit and sleep/resume; no persistent one-sided playback or missing virtual input during recovery. |
| Deep recovery | Full service reset passed 21 ordinary stops on 0.1.7; earlier KS timeout and sink-only reset 0/87 failed | Exercise the failure fallback and interrupted recovery. Ordinary stop success does not establish those cases. |
| Access denial | Installed-user access passed; other-user denial pending | Deny both pipes and control devices to another ordinary account; permit SYSTEM and the installed user. Recheck after upgrade. |
| Driver Verifier | Pending | Run relevant checks only against NTPodsAAP.sys and AudioCodec.sys while repeating I/O, removal and suspend/resume; save results and remove verifier configuration. |
| Upgrade/removal | 0.1.8 upgrade exposed a file-version regression; targeted Main repair passed; startup snapshot discrepancy, uninstall and failed-upgrade recovery remain open | Confirm startup choice through a real-account upgrade and complete removal of tasks, drivers and development certificates. A failed major upgrade can restore app files without restoring removed driver packages; test repair with the previous installer. |

Preserve test results with date, Windows/Intel driver versions, firmware, build
manifest, application input/output selections and observed timings. Keep recordings
local; daemon logs exclude microphone content. Do not mark the stereo
or microphone-stop checks passed based only on source inspection or synthetic tests.

### Post-reboot check on 2026-10-11

Windows restarted at **11:20:48 Europe/Amsterdam**. Both NTPods drivers were
Running and both devices reported problem code 0.
Alternative A2DP remained **1.8.0.1 / oem21.inf**. The installed 0.1.8 daemon and
UI DLL still matched the verified package. The user confirmed the app opened and
connected; the observed frontend started manually at **11:29:25**, followed by
its daemon. This does not establish automatic login startup.

The running app showed Start with Windows off and automatic microphone use off,
despite the separately readable Run entries and preference file showing on.
The daemon's runtime profile paths and user token matched the expected account;
the cause of the discrepancy was not established. Start with Windows was enabled
through the running UI, and automatic microphone use was restored through IPC.
After closing both processes while idle and reopening the installed app through
Windows Explorer, **both preferences remained on**, the AirPods reconnected, and
the microphone was available and idle without error. No recording or listening
test was performed because the buds were charging in their case. Automatic launch
at the next Windows login still needs confirmation. The readable canonical daemon
log remained unchanged; post-reboot findings came from live IPC, UI and system
queries rather than that log.

Local evidence: `ntpods-018-postboot-verification.json`,
`ntpods-018-postboot-preferences.json` and `ntpods-018-postboot-final-state.jsonl`
under `../../outputs/`. The earlier restricted-access check is retained separately.

### Apple-style interface preview on 2026-10-11

The Release WinUI build passed with the existing nullable warning in
`Popup/IslandView.xaml.cs`. The listening-selector test project passed geometry,
drag-transition, conditional-Off and command-safety checks. Native UI checks covered
light/dark themes, 806px and 446px windows, settings-notice dismissal, green enabled
switches, and keyboard focus on the embedded Adaptive strength control. Both pages
now center their 640-DIP content within a capped scroll viewport.

Live attempts to change Adaptive strength and select Transparency timed out without
device confirmation. Refreshing the device settings subsequently reported Adaptive
at 50%, the original state. The UI retained the reported state and displayed the
failure. These attempts do not validate hardware control changes.

This preview ran from the build output against the installed daemon. The redesigned
frontend was packaged as 0.1.9; the first elevation attempt was cancelled before
installation. Startup entries were not changed.
The System theme preference was restored after the visual checks.

### Microphone interruption fix on 2026-10-11

The user reported that removing the AirPods during recording stopped microphone
audio, and reinserting them while the recording remained open produced loud static
on 0.1.8. This is an additional hardware failure, separate from ordinary capture
start/stop and the earlier sustained capture checks.

LibrePods PR [655](https://github.com/librepods-org/librepods/pull/655), inspected at
`99b0e580a93f2a26647f89816895ca1e5f70a3ec`, recreates its capture pipeline after
two seconds without microphone packets. NTPods retained AAC decoder and resampler
history throughout an active Windows capture, including ear handoffs and packet
outages. That is consistent with the report, but does not prove its acoustic cause.

The first candidate dropped that history on stable ear-state changes and before
packets resumed after a two-second gap. The user repeated the recording test and
still heard static. The 41.296-second capture reported 1,346 packets, zero invalid
packets and 1,938,160 decoded/delivered frames. Packets continued during removal;
both removal and reinsertion triggered local decoder resets. A decoder reset alone
did not fix the failure. Ordinary stop completed its audio reset with codes 0/0
and returned to idle after 17.131 seconds.

The second candidate stops the physical stream when both buds are confirmed out,
uses the existing bounded playback recovery, and starts a fresh stream when at
least one bud returns and the app still holds capture permission. The both-out
state survives AAP reopening during recovery; transitional reports do not clear it.
The virtual input remains present and supplies silence. The UI has a
`waiting_for_ear` state after recovery. Reinsertion cannot clear a failed-start
latch or restart after capture closes or the feature is disabled.

Local decoder resets and queued-PCM flushes never reopen a revoked capture
permission. Invalid framing or AAC decode failures discard the whole packet's
output and decoder history. Flush failures enter the existing stop/error path.

All 51 daemon tests, three IPC tests, Rust/C# contract checks, and Release daemon
and UI builds passed. The user repeated removal/reinsertion in the same recording
and confirmed that speech returned without static. Removal triggered STOP and one
audio-service reset (codes 0/0); recovery completed in 16.865 seconds. A fresh
START followed, delivering its first PCM after 309 ms and another 13.427 seconds
of capture before the user stopped recording. Both capture segments had zero
invalid packets. Final state was connected and idle, with no active capture,
uplink or microphone error.

This validates one both-bud removal/reinsertion cycle with this candidate, whose
daemon SHA-256 is `E9F4AC0AD6566D2FA5EE660071D9557014EC581ADF73684AFF6324D94A6FF530`.
Single-bud handoff, repeated interruption cycles and acoustic stereo checks after
these interruptions remain pending. Ordinary end-of-capture recovery is unchanged.

### Installed 0.1.9 verification on 2026-10-11

The app-only MSI upgrade completed with exit code 0 and no restart request.
All 588 installed payload files matched the package manifest; both WinUI version resources report
0.1.9.0, and the installed daemon matches the hardware-tested candidate above.
The MSI SHA-256 is
`633159AEAF5F0C5271AB9C8092CD0208149BB46F9DE3D10C7954DD9E768D59B4`.

Both NTPods drivers retained their paths and hashes and were running afterward.
Alternative A2DP remained 1.8.0.1 / oem21.inf. The Drivers and MicAcx MSI features
remain registered as installed for future removal. The two Windows startup
entries still point to the installed app and daemon, and the installed UI shows
Start with Windows enabled. This does not verify another Windows login.

Local evidence: `ntpods-019-installed-files.json`,
`ntpods-019-postinstall-system.json`, `ntpods-upgrade-019-mic-result.json` and
`ntpods-upgrade-019-mic.log` under `../../outputs/`.

Post-install reconnection was initially blocked. The user confirmed the buds were in
their case during the initial check. Windows still reported a Classic connection,
but a one-shot AAP diagnostic returned `STATUS_DEVICE_NOT_CONNECTED`
(`0xC000009D`, Win32 1167) for both connect and disconnect. There was one AAP
device interface, and its bound address matched the selected target.

The AAP device subsequently stopped opening. The installed recovery task ran at
13:18:58 local time, after which Windows reported Device Manager Code 14
(`CM_PROB_NEED_RESTART`). The driver files were not replaced. The initial cause of
the reconnect failure is not established. The app and daemon were stopped cleanly
to end retries; startup entries remained enabled. A PC restart and a fresh connection
check were required before calling post-install validation complete.
Evidence: `ntpods-019-connect-diagnostic.json` and
`ntpods-019-restart-required.json` under `../../outputs/`.

### Early-login worker failure on 2026-10-11

The reboot cleared AAP Code 14. Windows Explorer automatically launched the
installed app and daemon at 13:27:43 local time, about 51 seconds after boot.
Both drivers reported OK, but the daemon stayed disconnected and its volume
remained zero. Its Bluetooth worker logged the local radio, then stopped logging
before opening the driver. Shutdown waited the full 60-second recovery deadline.
Relaunching the same installed files through Explorer restored the connection,
live battery readings and the idle microphone without a driver change.

Both the Bluetooth reconnect worker and playback worker initialized cooldowns
with `Instant::now() - Duration::from_secs(60)`. The pinned Rust Windows runtime
stores the performance counter as a duration; subtracting more than its current
value panics. The measured counter tracked boot uptime. These initializers could
therefore terminate both workers during the first minute after boot while IPC
and the tray kept running.

Version 0.1.10 represents an unused cooldown with `None`. First checks run
immediately; subsequent checks retain their five-second and three-second
intervals. All 52 daemon tests passed, including first-run and exact-boundary
checks for both intervals. The Release daemon and WinUI builds passed; the UI
retained its existing nullable warning. No driver code or packages changed.

The diagnostic environment resolved the usual user log path to a stale
package-private copy. The stale log was preserved in the local outputs directory;
removing that shadow allowed reads of the actual startup log. Process ownership,
the named-pipe server PID and the actual log confirmed the observations above.
Early-login behavior of the corrected build still requires a fresh boot check.

### Installed 0.1.10 verification on 2026-10-11

After administrator approval, the app-only upgrade completed at 15:49:52 local
time with Windows Installer status 0. The installed-file comparison checked all
588 payload files and found no missing or differing files. Both WinUI version
resources report 0.1.10.0. The installed daemon SHA-256 is
`AB9D4A28A6E61E6DE23DC7F2D2A851C942412261A841B62D0BD99470A04513C8`.

NTPodsAAP and NTPodsMicACX were Running after installation, with unchanged driver
paths and hashes. The installer skipped the driver-install action. Alternative
A2DP remained 1.8.0.1 / oem21.inf.

Launching the installed app through Windows Explorer opened one NTPods window.
Settings showed v0.1.10 and Start with Windows enabled. Live IPC confirmed a
connected, fresh link, left/right batteries at 60%/59%, and confirmed Adaptive
mode at 50% strength. The microphone was available and idle, with automatic use
enabled, no active capture or uplink, and no microphone or settings error.
Recommended EQ and automatic pause were enabled. Automatic connection and volume
updates during the first minute after a fresh boot remain pending.

Local evidence: `ntpods-upgrade-0110-retry2.log`,
`ntpods-0110-installed-files.json`, `ntpods-0110-driver-verification.json` and
`ntpods-0110-postinstall-state.jsonl` under `../../outputs/`.

### Adaptive strength diagnosis on 2026-10-11

The user confirmed that LibrePods Android rewrite changes the audible Adaptive
effect and retains the setting after reconnecting. Its strength command matches
NTPods' eleven-byte control-46 packet. Android displays the request immediately;
NTPods had required an exact echo, then timed out and restored its cached value.

With Adaptive mode confirmed, a bounded diagnostic used the
original version-1.2 handshake and FF feature mask. Fresh control connections
returned the following values:

| Requested | Reported after reconnect |
| --- | --- |
| 0 | 0 |
| 10 | 0 |
| 25 | 50 |
| 75 | 100 |
| 90 | 100 |
| 100 | 100 |

No strength write produced an immediate echo. Notification requests did not
refresh it, and repeating initialization on the same channel did not provide a
strength report. Earlier trials with version 1.3 and D7 did not establish a
benefit; those experimental changes were removed. The diagnostic requested the
original strength 50 and Transparency mode during cleanup; mode 3 was confirmed.
It did not reset audio services or replace drivers.

The fix separates a successfully sent strength from confirmed device settings.
A later valid report or connection reset clears the sent value. Other controls
retain their acknowledgement and timeout rules. The UI waits for Adaptive mode
confirmation before sending a strength chosen in the same drag, preserves the
requested thumb position, and permits another strength request after sending.
The tested A3063 / 9A348 firmware uses three slider steps; other model/firmware
combinations keep the continuous range.

Local protocol evidence: `adaptive-raw-ff-fresh-readbacks.json` and
`adaptive-raw-original-handshake.json` under `../../outputs/`.
All 55 daemon tests, four IPC tests, the Rust/C# contract checks (including 36
malformed-input cases), and selector tests passed. Release daemon and WinUI
builds passed; WinUI retains the pre-existing IslandView nullable warning.
Review found and fixed a stale-report race when restoring strength 50 after
silently sending 0. The MSI passed 40 startup-preservation and 18
driver-preservation cases.

The 0.1.11 preview opened and connected with fresh A3063 metadata, strength 50,
Transparency, and an idle microphone. During the subsequent UI check, requests
to switch to Adaptive were not acknowledged and a fresh connection still
reported Transparency. The user confirmed the buds were out of their ears.
After reinsertion, selecting Adaptive in the same candidate produced mode-4
readback. A native drag sent strength 0, a rail tap sent 100, and a second tap
restored 50 even though the last device report was still 50. The thumb retained
each selection and the controls remained available without timeout errors.
The user then moved between the endpoints and confirmed that the amount of
outside sound changed audibly. Their final choice before installation was 0.

Live evidence: `adaptive-worn-mode.jsonl`, `adaptive-worn-drag-zero.jsonl`,
`adaptive-worn-tap-hundred.jsonl` and `adaptive-worn-restore-fifty.jsonl` under
`../../outputs/`. The microphone remained idle during these checks.

The 0.1.11 MSI completed with code 0. All 587 MSI payload files matched the
distribution; the distribution marker and ZIP-only `install.ps1` are excluded
from the MSI. Driver feature registration and both startup paths were preserved.
Both running NTPods drivers retained their paths and hashes. Alternative A2DP
remained 1.8.0.1 / oem21.inf. Evidence is in `ntpods-0111-installed-files.json`,
`ntpods-0111-driver-verification.json` and `ntpods-upgrade-0111.log`.

### Continuous slider and quieter controls

Version 0.1.12 replaces the three-position restriction with Android's continuous
dragging. The reconnect tests above establish coarse reports, not a limit on the
audible effect's resolution. The UI preserves the full range and requested values
independently from those reports.

The 0.1.12 selector keeps a continuous pointer position and coalesces live
requests every 100 ms, with one request outstanding and one replaceable target.
Release retains the final target until it can be sent. Adaptive mode must be
confirmed before a strength write. Mode selection still works when strength
has not been reported, and an unconfirmed target cannot leave the controls
disabled indefinitely.

The banner below the batteries and its event subscription are removed. Routine
confirmation and pending text is now accessibility help or a tooltip. The EQ
header retains its reported preset during edits. Active microphone, recovery
and actionable error messages remain visible.

Selector regression checks passed for geometry, all 101 strength values,
coalescing, final-value delivery, stale readback, cancellation and mode ordering.
The Release WinUI build passed with the existing IslandView nullable warning.
Review found no remaining code blocker. Native dragging remains pending.
Native interaction testing was interrupted. A post-install visual check
confirmed that the banner and routine status lines were absent. Mode widths still
change immediately; Android's spring animation is not implemented.

The 0.1.12 MSI was built from the updated UI and unchanged 0.1.11 daemon and
driver packages. It passed all 40 startup-preservation and 18 driver-preservation
cases. Its SHA-256 is
`CFB1795EEB881DEA5AE9FB89C2FCF6A708C9F6E1C82BCAD8CD0F48D77EB18789`.
The manifest records all 589 distribution files. The MSI completed with code 0,
and all 587 installed payload files matched the package. Both startup entries remain enabled and point to Program
Files. The NTPods drivers retain their paths and hashes and are running;
Alternative A2DP remains 1.8.0.1 / oem21.inf. The installed app and daemon opened
from Program Files and connected with a fresh link and idle microphone.
Evidence: `ntpods-upgrade-0112.log`, `ntpods-0112-installed-files.json`,
`ntpods-0112-driver-verification.json` and `ntpods-0112-after-install-state.jsonl`
under `../../outputs/`.

### Motion, scrollbars and island popup (0.1.13)

The selector retains its icon images and moves the selected capsule without
reloading artwork. Mode transitions settle from their current visible position;
pointer dragging remains direct. The user confirmed smooth dragging with no
flashing. Selector and island motion regression checks pass, including interrupted
transitions, repeated notifications and reduced motion.

Native checks on both Device and Settings pages showed the scrollbar during
scrolling and hidden after idle. It waits 400 ms, then fades over 120 ms while
retaining native hit targets and keyboard access.

The first island preview was visible but appeared on the wrong monitor and kept
a rectangular Windows frame around its pill. The revision uses the primary
monitor, removes residual dialog-frame and window-edge styles,
uses primary-monitor DPI, and follows LibrePods' expanded layout with combined
buds artwork and the battery number inside its ring. Light and dark appearance,
large text, high contrast and reduced motion have explicit handling.

The final Release build passed without warnings. Native interaction testing
ended before the revised popup launched, leaving its frame, placement and
expanded layout unverified. This candidate has not
replaced the installed 0.1.12 app. The backend and driver packages are unchanged;
audio hardware tests were not repeated for these UI changes.

The 0.1.13 MSI passed all 40 startup-preservation and 18 driver-preservation
cases. Its manifest records 590 distribution files and MSI SHA-256
`F8E6356BC17454013FF6B4746DC59DE35B108C9C64DDAAA4E1B90993E68A2082`.

The MSI completed with code 0,
and all 588 installed payload files matched the distribution. Both startup
entries remain enabled and point to Program Files. Both NTPods drivers are
running with unchanged paths and hashes; Alternative A2DP remains 1.8.0.1 /
oem21.inf. The installed app opened and connected with fresh battery readings,
Adaptive mode and an idle microphone. Its main page was visually checked;
the revised transient popup's appearance still needs confirmation.
Evidence: `ntpods-upgrade-0113.log`, `ntpods-0113-installed-files.json`,
`ntpods-0113-driver-verification.json` and
`ntpods-0113-after-install-state.jsonl` under `../../outputs/`.

## 0.1.14 parity and bug review (2026-10-11)

The 0.1.13 popup passed visual review. The 0.1.14 build and installation checks
are recorded below; its new audio behavior still needs live validation.

Compared the pinned LibrePods rewrite, main branch and popular/relevant forks in
[`WINDOWS-UPSTREAM-REVIEW.md`](WINDOWS-UPSTREAM-REVIEW.md). Added a saved Windows
Conversation Awareness volume limit. Fixed missing conversation-end statuses,
restoration to the wrong output, restoration over manual volume changes, and
volume left lowered after the control session ends.

Other fixes cover public firmware parsing, malformed ear/battery reports,
explicitly absent battery components, stale EQ/settings drafts, pending EQ
timeouts, hidden Off selections, and retrying an unconfirmed listening mode.

Explicit Disconnect now sends microphone STOP and closes AAP before disabling
audio profiles. A subsequent Connect performs one conservative audio-service
rebuild, even if a newer request superseded the original Disconnect. Newer user
intent wins, and failed recovery leaves a visible error and an explicit retry
requirement. This can make an explicit reconnect take longer; its timing and
stereo behavior need hardware validation. Ordinary capture-stop recovery and
shutdown still use the previously tested lifecycle.

Validation on the final source:

- 72 daemon tests and 5 IPC tests passed.
- Rust/C# production/legacy roundtrips and 42 malformed wire inputs passed.
- DeviceSettings, ListeningMode, IslandMotion and Startup regression programs
  passed. DeviceSettings is included in the standard build script.
- WinUI and static-runtime Rust Release builds passed.
- MSI checks passed 40 startup-preservation and 18 driver-preservation cases.
- The 590-file distribution's daemon matches the final Release binary. Both
  driver packages match the 0.1.13 distribution byte for byte.

MSI SHA-256:
`BE74BB5DE2EB83D4C85B7504273B4D5ECA3B4CE9B602B2B225B5DF55441DFA51`.
The package and manifest are in `windows/build-output/package-0.1.14` from the
repository root. Build and test logs are under `../../outputs/ntpods-0114-*`.

Pending live checks: the new slider and timeout behavior, volume restoration
during a conversation/output switch, public firmware display, case battery
absence, and Disconnect/Connect/Repair while an application holds capture open.
Repeat the stereo stop/start and ear-removal checks before promoting this build.
No drivers, codec settings, startup entries or installed files changed during
the build and review pass.

The app-only MSI upgrade completed
with exit code 0 and no restart required. All 588 installed payload files match
the distribution; `install.ps1` and the distribution marker are intentionally
excluded from installation. Both NTPods drivers remain running with unchanged
paths and hashes. Alternative A2DP remains 1.8.0.1 / oem21.inf. The NTPods startup
entry is unchanged and points to the installed UI with `--tray`; no separate
daemon Run entry existed before or after this upgrade.

The installed UI and backend opened from Program Files and reconnected. Fresh
battery readings, confirmed Adaptive mode, EQ settings and an idle, available
microphone were observed. The main page was visually checked. The information
parser still reports an internal firmware value (`9442752`) rather than the
known public version 9A348, so public firmware display remains a known bug.
Conversation volume and microphone recovery tests were not repeated during
installation. Evidence under `../../outputs/`: `ntpods-upgrade-0114.log`,
`ntpods-upgrade-0114-result.json`, `ntpods-0114-installed-files.json`,
`ntpods-0114-drivers-before.json`, `ntpods-0114-drivers-after.json`,
`ntpods-0114-startup-before.json`, `ntpods-0114-startup-after.json` and
`ntpods-0114-after-install-state.jsonl`.

## 0.1.15 Allow Off fix (2026-10-11)

Investigated the unavailable switch against LibrePods' Android rewrite. Android
permits this request based on model support and retains control values in its
per-device cache. NTPods incorrectly required a first `0x34` report. On this
Pro 3 pair, the enable request produced no report, while selecting listening
mode Off produced a confirmed `0x0D=1` response. Adaptive was then restored.

The fix permits Allow Off requests on known Pro 3 models, keeps the sent choice
separate from confirmed controls, and saves it per device. Actual readback wins
over the cache. Reconnect restores the choice without replaying a write. The
main selector, tray and stem-cycle options share the same effective choice.
Other unreported controls retain their existing gates.

Validation passed: 76 daemon tests, 6 IPC tests, Rust/C# roundtrips with 44
malformed inputs, DeviceSettings and ListeningMode checks, and Release builds.
The candidate ran from its distribution directory. Its native switch was
available and enabled successfully; there was no false confirmation timeout.
Both enabled and disabled choices survived control-channel reconnects. Off
appeared in the selector and its selection was device-confirmed. Adaptive was
restored, with Allow Off enabled. Microphone capture and driver tests were not
repeated for this settings change.

The candidate initially ran for review while 0.1.14 remained installed.
Evidence is under `../../outputs/ntpods-allow-off-*` and
`../../outputs/ntpods-0115-*`. The existing driver packages are reused unchanged.
The 0.1.15 MSI passed 40 startup-preservation and 18 driver-preservation cases;
all six driver-package files match 0.1.14. Its 590-file distribution and hashes
are recorded in `windows/build-output/package-0.1.15/build-manifest.json`.

The app-only upgrade completed with exit
code 0 and no restart required. All 588 installed payload files match the build.
Both NTPods drivers remain running with unchanged paths and hashes; Alternative
A2DP remains 1.8.0.1 / oem21.inf. The startup entry is unchanged. The installed
UI and backend opened from Program Files, reconnected, restored the saved Allow
Off choice, and reported confirmed Adaptive mode with an idle microphone and
no setting errors. The enabled switch was checked visually. Evidence:
`ntpods-upgrade-0115-result.json`, `ntpods-0115-installed-files.json`,
`ntpods-0115-driver-verification.json`, `ntpods-0115-startup-after.json` and
`ntpods-0115-after-install-state.jsonl` under `../../outputs/`.

## 0.1.16 microphone and driver candidate (2026-10-11)

This is a source/build candidate. The installed application remains 0.1.15 and
the installed drivers have not been replaced. The research is recorded in
[WINDOWS-MIC-PROTOCOL-RESEARCH.md](WINDOWS-MIC-PROTOCOL-RESEARCH.md), with pinned
LibrePods revisions, related implementations, and separate hardware claims.

The AAP driver now routes control, AAP reads and ATT reads to independent queues.
The backend uses overlapped I/O so the operating system does not serialize the
same file handle first. Each request owns its cancellation context; close waits
for the affected receive before releasing its channel. Existing ACLs, IOCTL
formats, peer restrictions, MTU and input queue depth are preserved. Lower-stack
cancellation still depends on bthport completing the request; a deadline cannot
make it safe to free buffers that the lower stack still owns.

The ACX driver corrects time/byte arithmetic after delayed callbacks, queues
overdue work without recursion, publishes packet/timestamp pairs together and
keeps a 64-bit internal packet count. Position queries and clock/state changes
share a brief lock; timer stop/start and ACX calls stay outside it. Capture exposes one hardware stream for
Windows' shared audio engine. It supports two event-driven packets and explicitly
rejects the single timer-driven buffer mode that this engine does not implement.
Normal shared-mode multi-app capture requires retesting with the new driver.

The backend stops capture after five seconds without decoded PCM delivered to
the bridge. It silences and flushes the input, follows the existing bounded
playback-recovery sequence and requires an explicit retry. Silent PCM counts as
progress. Decoder validation rejects unexpected formats, corrupt flags and
non-finite samples, avoids modifying shared decoder buffers and checks resampler
capacity. The inherited 64 kHz presentation-clock assumption, 48 kHz mono output,
gain and jitter-buffer targets are unchanged. AU timestamp diagnostics record
aggregate deltas and counts only; they do not alter playback or log audio.

The capture monitor also wakes the receiver's idle wait when capture changes,
removing up to 150 ms of avoidable scheduling delay. Recovery still blocks a new
START until stereo restoration completes. Per-service Windows call durations
and audio-lock wait times are logged to locate that longer delay. These timings
contain no device addresses or microphone payloads. Startup and recovery latency
have not been measured with this candidate.

Validation passed: 81 daemon tests, 6 IPC tests, Rust/C# roundtrips with 44
malformed inputs, the production-IOCTL concurrency harness, two real Windows
overlapped-I/O tests, mic ring/SID/timing checks and FFmpeg conversion checks.
The complete build also passed the existing WinUI policy and setup tests, both
WDK Release builds, catalog generation, and 40 startup / 18 driver-preservation
MSI cases. The final clock-lock change received a separate clean WDK rebuild
and independent source review. These are host-side checks, not loaded-driver
acceptance. Evidence is in `../../outputs/driver-hardening-*`; the unsigned
development package and source/payload manifest are under
`windows/build-output/package-0.1.16/`.

Both installed NTPods driver hashes still match the 0.1.15 installation baseline.
No drivers, boot settings or certificates were changed, and no microphone audio
was recorded in this pass. This candidate includes new driver binaries, so the
app-only upgrade flag must not be used when testing those improvements.

Required hardware checks remain: actual AAC-ELD frames accepted on 9A348;
simultaneous Sound Recorder/Discord capture; twenty start/stop and ear-removal
cycles; a sustained capture with stereo separation, latency and drift checks;
packet loss, sleep/resume, device removal and daemon crash. Driver Verifier and
ACL checks belong on the prepared driver test setup. Temporary device-level
Conversation Awareness ownership and a faster Windows stop/recovery sequence
remain unresolved. Finite decoded samples alone cannot prove static is absent.

## 0.1.17 audit fixes and microphone controls (2026-10-11)

This candidate addresses the thirteen audit findings and the smaller UI issues.
Bluetooth status reads no longer wait behind ATT writes; setting writes release
the shared state locks while preserving confirmation order. Microphone permission
changes are serialized with recovery. Discovery retries when Windows has no
paired AirPods at startup, and playback detection distinguishes ordinary audio
sessions from system sounds.

Quit waits for command delivery and backend exit. Both IPC pipes belong to one
session; losing either disables stale controls and discards queued commands.
Heart-rate enable state and sample timestamps come from the backend, and OFF
cannot be followed by an older worker's START. Hearing settings require a valid
ATT response and suppress results from superseded commands or connections.
Rename checks the UTF-8 byte limit and reports a sent request rather than a
confirmed device rename. The tray menu chooses the cursor's monitor before
measuring itself, and unavailable microphone access no longer implies a missing
driver.

The ACX endpoint leaves volume control to Windows and limits packets to the
100 ms size its PCM ring supports. The obsolete microphone installer now stops
with instructions to use the complete package. Driver setup stages replacements
before unbinding, retains exported packages until commit, and restores them on
failure. Fault tests exercise the production installer functions with mocked
privileged operations; they do not prove Windows can recover every PnP failure.

The microphone modes add automatic idle timeout, Keep active and
manual Start/Stop. Automatic immediate shutdown remains the default. Saved
preferences exclude manual start intent. Removing both AirPods, disable and disconnect
override retained streams, and audio received without a recording client is
discarded. The existing stereo recovery sequence remains in place.

The full build passed 102 daemon tests, 8 IPC tests, the Rust/C# contract and
client-session checks, both WDK builds, native driver tests, 13 installer fault
scenarios under PowerShell 5.1 and 7, and 40 startup / 18 driver MSI cases.
Installation completed with a restart request. All 589 installed payload files
matched the build; both driver images matched after excluding signing metadata.
Alternative A2DP and both startup entries were preserved.

Live verification found a microphone startup failure (Code 10,
STATUS_INVALID_DEVICE_STATE). The new packet-constraint callback queried the
single-circuit symbolic link before AcxDeviceAddCircuit had made it available.
The installer also accepted a present microphone device without checking its
problem code. This version did not pass loaded-microphone validation.

## 0.1.18 microphone startup correction (2026-10-11)

Packet constraints are applied after AcxDeviceAddCircuit. Installation checks
the microphone device problem code and rolls back failed starts; a genuine
restart-required state remains allowed. The complete build passed again, now
including 18 driver rollback scenarios under PowerShell 5.1 and 7.

0.1.18 is installed. All 589 payload files matched the package. Both NTPods
drivers are running and their signed images match the built binaries after
excluding signing metadata. The microphone device has problem code 0 and an
active 48 kHz mono endpoint (16-bit device format, float32 Windows mix format).
Hardware volume and mute flags are clear, allowing Windows software controls.
Alternative A2DP remains on its original package/version, and both Start with
Windows entries remain enabled. Windows requested a restart; it was not forced.

Short live checks passed with the installed app and drivers. Manual Start opened
the uplink without a recording client; Stop supplied silence to a client that
kept recording, then completed recovery. Keep active retained the uplink after
the client closed. Automatic with a five-second delay retained the uplink during
the idle interval, then stopped and recovered. Three WASAPI captures (25, 5 and
18 seconds) delivered 48 kHz PCM with no non-finite samples; each reported one
initial discontinuity and none afterward. Samples were used only for in-memory
statistics; no recordings were saved. Automatic mode with immediate stop was
restored, and the final state was connected, idle, with no microphone error.

Evidence: `../../outputs/ntpods-0118-*` and
`../../outputs/ntpods-upgrade-0118-*`. The installer SHA-256 is
`660628480ECCF1D51572078D95F6080AFBF8E76CD3AEB3DB8FC15B0E5AD2280B`.
Post-reboot checks, audible volume attenuation, sustained stereo recording,
multi-app capture, ear-removal recovery and Driver Verifier remain pending.

## 0.1.19 battery island

The source adds paired BLE battery reports, independent Left/Right/Case island
readings, charging indicators, staged capsule motion, and Show batteries in the
tray. Cached values are dated and no longer look like live charging. Battery
updates do not extend the popup lifetime. Centered charging paths also replace
the main battery display's misaligned icon controls.

The coordinated build passed 110 daemon tests, eight Rust IPC tests, Rust/C#
roundtrips, native driver/decoder checks, client sessions, device settings,
motion, installer rollback, 40 MSI startup cases and 18 driver-preservation
cases. A timing race in the mismatched-process pipe test was fixed by waiting
for its child server to begin accepting; production PID validation was unchanged.
The final UI build passed after the charging-path correction. Isolated native
captures checked light/dark, charging, unknown, zero, cached and long-name cases.

The battery parser has independent AES and private-address fixtures, current-user
DPAPI tamper checks, freshness expiry, and reconnect-latch regression coverage.
Installation passed with all 589 payload files matching the package. Both loaded
drivers still match 0.1.18, and Alternative A2DP and startup entries were preserved.
Real AX200 advertisements resolved to the paired AirPods and decrypted successfully:
one lid cycle produced 125 battery reports, including case 100% and both buds
charging. The reported lid flag stayed open, so these frames do not prove fresh
updates after lid closure. Windows independently connected audio while the lid
was open. The idle-only scan missed reports during that connection, and a
15-second charging guard expired before Windows disconnected 20 seconds later.

## 0.1.20 battery scanning during playback

A separate one-minute passive scan received 1,064 advertisements, 247 Apple
frames and 55 identity-matched, decrypted battery reports while music played.
The user reported smooth playback with other Bluetooth devices disconnected.
Both worn buds reported 74%; the case value was absent. No microphone capture
ran during this test. Evidence is in `../../outputs/ntpods-0120-passive-playback.txt`.

The follow-up adds a saved opt-in for scanning during playback, retains the
microphone exclusion, and extends the recent-charging guard to 45 seconds.
Cached BLE samples retain their original timestamps and cannot replace newer
AAP readings. Fresh BLE also supplies battery readings when the control channel
has gone quiet during playback; it does not change the audio connection state.
Connected advertisements retain the arrival latch to prevent false reconnects
after a phone handoff.

115 daemon tests, eight Rust IPC tests, nine Rust/C# state fixtures, 63 malformed
inputs and 18 commands passed. The Release backend and WinUI builds passed, as
did an independent review of timestamp handling, lock order and reconnect logic.
Driver sources are unchanged from 0.1.19; this package reuses those built driver
packages and the app-only installer preserves the loaded 0.1.18 drivers.

With the opt-in enabled, the production listener handed off from AAP to a newer
BLE report when Windows disconnected audio at 19:51:40 UTC. It received another
BLE report at 19:52:36 UTC, after the user's closed-lid acknowledgement at
19:51:51 UTC. Both buds reported 74% and charging; the case reported 100% and not
charging. No audio reconnect or reclaim attempt followed. The reports paused for
about 59 seconds, so battery freshness was increased to 90 seconds while arrival
and case-reclaim timing stayed at 45 seconds. Regression tests cover fresh at
60 seconds and expired at 90 without changing the sample timestamp. The later
observation contained no further updates. Evidence is in
`../../outputs/ntpods-0120-final-lid-cycle.jsonl` and
`../../outputs/ntpods-0120-closed-extension.jsonl`.

This validates closed-lid report reception and audio separation on AX200/9A348.
It does not establish the earbuds' sleep duration, charging-percentage accuracy
over time, coexistence with other Bluetooth devices, sustained playback, or
sleep/resume behavior. macOS's exact cache and expiry policy was not traced.

0.1.20 installed successfully with all 589 installed payload files matching the
package. Both loaded NTPods drivers, Alternative A2DP 1.8.0.1 and the two startup
entries were preserved. The installed daemon reloaded the enabled playback-scan
preference. MSI SHA-256:
`206D25952B15069C0809762FB1B36AECD89E3C939B3AA9CA8E511219F2A26F9D`.

After the case had gone quiet, the user plugged in its charger while leaving the
lid closed. The installed listener received no battery update. An independent
60-second passive scan remained started and received 754 advertisements, including
200 Apple frames, but zero proximity records or matches to the paired AirPods.
This test found no charger-triggered battery broadcast and could not verify a
change in the case charging flag. Audio remained disconnected. Evidence is in
`../../outputs/ntpods-0120-charger.jsonl` and
`../../outputs/ntpods-0120-charger-probe.txt`.

## Unreleased island frame pacing

The island now advances on WinUI rendering frames while its capsule, message
or rings are moving. It unsubscribes while holding or hidden and uses a
one-shot timer for dismissal. Unchanged rounded window bounds no longer cause
another native region update. Appearance and animation durations are unchanged.

Separate baseline and candidate previews ran four identical notification cycles
without opening daemon pipes or touching audio. Excluding the first warm-up
cycle, median animation-update spacing fell from 31.142 ms to 6.936 ms (about
32 to 144 updates per second); the 95th percentile fell from 32.283 ms to
7.569 ms. Mean render-call work fell from 1.277 ms to 0.612 ms. XAML Rendered
callback spacing fell from a 19.507 ms median to 7.284 ms. These are application
timings, not a measurement of physical display presentations or a guaranteed FPS.
Raw evidence is in `../../outputs/island-fps-baseline-0121.json` and
`../../outputs/island-fps-candidate-0121.json`.

The rendering hook was detached during hold and after hiding in all four cycles.
The candidate's isolated native preview retained the artwork, labels, rings and
centered charging bolt. Release WinUI build and motion tests passed, including
60/120/144/240 Hz timing, interrupted dismissal and reduced motion. Independent
review found no new lifecycle defect. The installed 0.1.20 app remains unchanged.

## 0.1.21 audit fixes

Installer preflight now stops the installed interface, verifies the daemon pipe's
process, requests shutdown and waits for recovery before old-product removal.
It aborts on timeout or a nonzero daemon exit. Restart Manager cannot terminate
recovery early. The new daemon reports failed or unfinished shutdown cleanup as
an error. The old 0.1.20 daemon does not, so this first upgrade still depends on
its existing recovery completing successfully.

Shutdown also freezes new audio requests, waits for active service resets and
receiver cleanup, then closes the service gate. Queued ordinary workers cannot
restart audio work afterward. Saved disconnects finish before receiver cleanup
is reported complete. A stalled service call produces a nonzero exit at the
deadline; the existing manual Restore audio failure reporting is unchanged.

Connection requests carry a generation: a finishing reconnect worker cannot
clear a newer request. Auto-resume observes media-session changes from before
the ear-removal pause and relinquishes control after manual playback changes.
Another playback-info change can also cancel automatic resume.

Each battery component now retains its own observation time. Buds-only reports
cannot refresh an old case percentage or charging flag. Cards, island, tray and
navigation distinguish cached readings. The later case-cache fix below retains
absent components' last readings and original observation times as stale.
Volume snapshots no longer override an active edit. Tray menus scroll within the
monitor work area and Escape dismisses them. Unused toast/balloon code and dormant
microphone sample render, DRM, tone and fake peak-meter code have been removed.

The integrated build passed for the backend, WinUI and both drivers. Native
driver/decoder checks, eight Rust IPC tests, nine state fixtures, 68 malformed
inputs, 18 commands and all five C# suites passed. Setup/shutdown and driver
rollback checks passed under PowerShell 5.1 and 7; MSI checks passed 40 startup
and 18 driver-preservation cases. The final backend change passed all 125 daemon
tests, including concurrency tests that pause a reset between disable and enable.
Native isolated previews confirmed per-component battery dimming, hidden stale
charging indicators and centered live charging paths. These previews did not
connect to the daemon or change Bluetooth. The tray had a 280-pixel viewport for
403 pixels of content. Direct wheel and Escape checks could not be completed
because native window targeting was unreliable; layout policy tests passed.
Evidence is in `../../outputs/audit-ui-native-validation-0121.json` and
`../../outputs/audit-ui-battery-0121.png`. Hardware lifecycle checks and driver
loading remain pending for this version; the installed 0.1.20 app is unchanged.

## 0.1.21 live movement and appearance

The head-tracking view uses LibrePods' two-channel movement graph. Its parser
checks the RTBuddy header, protobuf fields, service and 58-byte record; malformed,
fragmented and combined packets have regression coverage. Samples use a separate,
bounded-rate event. One sensor-control worker keeps Bluetooth writes off the
microphone receive path. Failed stop requests retain their cleanup obligation
and retry once; stale command clients cannot cancel a newer owner's request.

The native graph checks passed with 100 samples per trace in dark, light and
280-pixel layouts, including clipping, rapid close/reopen command ordering and
event/render-hook cleanup. Evidence is in
`../../outputs/head-tracking-native-0121.json` and the separate light-theme run
`../../outputs/head-tracking-native-light-solo-0121.json`. The display is raw movement,
not a calibrated pose, and has no gesture actions or spatial-audio output.

Nearby battery updates retain known model artwork even when the BLE report
omits model metadata. The same bitmap remains loaded across repeated reports;
genuine image changes use short compositor fades. Native checks covered retained
images, stale component styling, show/hide transitions and unload cleanup in
`../../outputs/artwork-native-0121.json`. Window entrance uses a short compositor
scale/fade and respects reduced motion.

The two installed scheduled helpers now enter through the windowless daemon and
start PowerShell with CREATE_NO_WINDOW. Only the fixed rename-mic and fix-driver
commands are accepted, with installed-path checks. A harmless real PowerShell
probe verified no console handle and preserved exit status. Setup waits for these
helper invocations without treating them as daemon sessions. Existing task
registrations change only when the updated package is installed.

The combined build passed 136 daemon tests, nine Rust IPC tests, nine state
fixtures, 80 malformed inputs, 20 command roundtrips, all five C# suites, native
driver/decoder checks, setup tests under PowerShell 5.1 and 7, and the 40 startup
and 18 driver-preservation MSI cases. Both drivers compiled and catalog checks
reported no errors or warnings. All 592 payload files matched the build manifest.
The package is `windows/build-output/features-0.1.21/NTPods-experimental.msi`,
SHA-256 `3A1C6EF634829F277F66C534D8E20AD07735D73595BB2DADBCF4889D195076B6`.

The live preview connected to the AX200 and AirPods Pro 3 through Windows. It
received 32 movement events in a five-second observation,
and the user confirmed that turns and nods moved the graph. Pause returned to idle
with zero movement events during the next five seconds. A separate command client
restarted tracking and received 33 samples; closing that client returned to idle
with zero published samples over the following five seconds. These observations
verify application delivery and cleanup; they do not measure radio traffic after
STOP. The microphone stayed idle throughout, so simultaneous recording is still
unvalidated. Evidence is in `../../outputs/ntpods-0121-head-*.json`.

The preview shut down with exit code 0 and the installed 0.1.20 app was restored.
The 0.1.21 package has not been installed. New helper task registrations, app
entrance after an installed launch, and prolonged sensor/microphone coexistence
still need installed/hardware checks. Sensor writes start cancellation after two
seconds; native completion must drain safely, so a stalled Bluetooth stack can
delay final completion beyond that threshold.

The live check exposed a case-cache bug: explicit unknown AAP values, partial BLE
reports and the next connection could erase an earlier case percentage. The fix
retains each component's last value and original timestamp, clears its charging
flag on an explicit absent report, and prevents unrelated reports from making it
live again. Reconnecting the same pair invalidates freshness without deleting the
values. The cache lasts for the daemon session. All 139 daemon tests passed after
this fix, including AAP absence, BLE/AAP transitions and reconnect regressions.

The corrected preview passed the case sequence: a fresh 95% case report
remained 95% after the bud was removed. Its original observation timestamp stayed
unchanged, `live` became false and charging was false while both buds had fresh
readings. The user confirmed that the percentage remained visible. Evidence is in
`../../outputs/ntpods-0121-case-live.jsonl` and
`../../outputs/ntpods-0121-case-after.jsonl`. The corrected preview remains running;
the installed files and startup registration are still 0.1.20.

The final installer is
`windows/build-output/features-0.1.21-final/NTPods-experimental.msi`, SHA-256
`F927EA38E16F8FFB6EE33A42B6D0E776E7DCACF17B7E9A83F5FCD7B5EBD2EBC9`.
It contains the rebuilt backend and the verified, unchanged UI and driver payload
from the full build. The new MSI again passed all 40 startup and 18
driver-preservation checks. The final backend hash is
`55D0649C6DBDFA2BE94DBB3ECF7A1CDA62A63BD4733AC1362CA59227716AF54A`.

## 0.1.21 listening selector and menu motion

The listening selector uses the pinned LibrePods rewrite's lens equations,
press/stretch behavior and interruptible springs. Win2D renders the lens; native
buttons, strength slider and focus indicators retain input and accessibility.
Covered Adaptive markers fade in both the native layer and the lens source.
Their coverage includes the unscaled source during the release rebound, avoiding
colored marker fragments when the capsule briefly compresses.

The isolated native preview passed dark/light, both Adaptive endpoints, a
268-DIP track, Off availability, forced high-contrast fallback and reduced motion.
All nine normal glass instances initialized without rendering errors. Native
keyboard focus was visible, and synthetic strength requests retained one pending
write and coalesced 73 to the final 81. Decoration and mode-snapshot animations
sent zero commands. Active callbacks had a 6.94 ms median interval; this measures
callback timing, not displayed frame rate. Settled and unloaded controls had no
remaining frame callbacks. Evidence is in
`../../outputs/listening-glass-native-0121.json` and its matching image exports.

The shared expander now reveals natural-sized content inside one rounded outer
surface. Native checks covered opening, closing, reversal, narrow layout, reduced
motion and unload/reload. The closing Heart Rate switch retained its green fill;
pointer input was blocked and attempted focus inside closing content returned to
the native header. Native collapse events kept their original timing. These
checks used simulated state without daemon or Bluetooth access.

After the user reported a hitch when dragging across modes, a separate native
profile compared the original renderer with retained glass resources and one
visual update per rendering callback. During five seconds of synthetic rapid
input, callback p99 decreased from 14.15 to 8.03 ms and draw CPU p95 from 1.18 to
0.79 ms. The final run processed 5,720 input updates with 698 full visual renders.
Two callback gaps near 18 ms remained. These short runs measure CPU submission
and callback timing, not GPU completion or guaranteed displayed frame rate.
Baseline and final measurements are in
`../../outputs/listening-glass-profile-baseline-0121.json` and
`../../outputs/listening-glass-profile-final-0121.json`.

Native burst checks retained all pointer movement and the latest position.
Releasing before the queued frame sent the final strength once; release and
cancellation cleared pending drawing work and returned to idle. The full
ListeningMode suite and the Release WinUI build passed after these changes.
Burst evidence is in `../../outputs/listening-glass-burst-checks-0121.json`.

The preview containing multiple selectors still hitched during manual dragging.
After installation, manual dragging was reported smooth in the normal app with
one selector.

The updated installer is
`windows/build-output/features-0.1.21-polish/NTPods-experimental.msi`, SHA-256
`9981605CA1EFE854B3102F649E7F667A5F290BBA7A0A51E0F5EA72095AFCC713`.
It passed 40 startup-preservation and 18 driver-preservation MSI checks. Its 597
payload files include the new graphics dependency and licensing notices. It
reuses verified unchanged backend and driver binaries from the preceding build.
The app-only upgrade completed with MSI
exit code 0. All 596 installed payload files matched the build manifest (the zip
installer is excluded). Both NTPods driver hashes and Alternative A2DP 1.8.0.1
remained unchanged. Start with Windows stayed enabled, and both scheduled helpers
now call `ntpodsd.exe --run-helper` instead of launching PowerShell directly.
The app and daemon started from Program Files and reconnected to the AirPods with
a fresh control link, current bud batteries, no device-setting error and an idle
microphone. The normal app window was inspected, and the user subsequently
confirmed smooth manual slider movement. Installation evidence is in
`../../outputs/ntpods-0121-polish-after.json`,
`../../outputs/ntpods-upgrade-0121-polish-result.json` and
`../../outputs/ntpods-0121-after-install-state.jsonl`.

## 0.1.22 microphone submenu and disconnected selector

Microphone settings now use the shared, initially collapsed expander. Recording,
recovery and error messages remain in its header. The controls and their handlers
are unchanged. Disabled listening-mode buttons use transparent backgrounds so the
disconnected selector keeps one continuous rail with dimmed icons. An unknown
mode does not show a selection.

An isolated native preview passed light/dark themes at 640 and 316 DIP widths.
All microphone controls were present; automatic, manual and keep-active settings
showed the appropriate controls. Recording and recovery remained visible while
collapsed. Opening and closing sent no commands. Both disconnected selector
checks found zero filled native button backgrounds and no enabled mode controls.
The preview used simulated state and closed itself without restarting the installed
app or accessing Bluetooth. Evidence is in
`../../outputs/mic-expander-check-0121.json` and matching image exports.

The Release WinUI build passed. The installer is
`windows/build-output/features-0.1.22/NTPods-experimental.msi`, SHA-256
`7808ACBAAC2FE5DA3AC81557130F2970192128C756722A1BB53035DBECD71465`.
It passed all 40 startup-preservation and 18 driver-preservation checks and reuses
the verified, unchanged backend and driver payload.

The app-only upgrade completed on
2026-10-12 with MSI exit code 0. All 596 installed payload files matched the build
manifest. Both NTPods drivers remained running with unchanged hashes and valid
signatures; Alternative A2DP 1.8.0.1 and Start with Windows were preserved. Both
scheduled helpers still call the installed daemon directly. The installed app
and daemon started from Program Files. The normal window showed the continuous
disconnected rail and collapsed microphone submenu. The AirPods were disconnected
and the microphone was idle, so this installation check did not exercise live
audio. Evidence is in `../../outputs/ntpods-0122-after.json`,
`../../outputs/ntpods-upgrade-0122-result.json` and
`../../outputs/ntpods-0122-after-install-state.jsonl`.

## 0.1.23 listening-label layout

Listening-mode labels now keep stable wrapping widths while render transforms
move them with the rail. The Off label fades before its segment becomes narrower
than its text box. This prevents animated wrapping from changing the label row's
height and moving the sections below it. Mode input and Bluetooth handling are
unchanged.

An isolated native comparison reproduced 16–128 DIP jumps in the previous layout.
All 20 fixed scenarios had zero change in label height, card height and the next
section's vertical position across 2,981 layout samples. Cases covered light/dark,
640/316/240 DIP widths, English/Portuguese/French/Spanish, Off enabled/disabled,
Adaptive transitions and rapid reversals. Labels remained within their measured
row, animation callbacks stopped when settled, and no device commands or errors
were recorded. The mock preview closed successfully without touching the installed
app or hardware. Evidence is in `../../outputs/listening-label-layout-0123.json`
and matching image exports.

The Release WinUI build and all 40 startup-preservation and 18 driver-preservation
installer checks passed. The package is
`windows/build-output/features-0.1.23/NTPods-experimental.msi`, SHA-256
`B173E7EFDF9927FF0FB7CB60D54B152E489B0179EE744E948B44343ECF60ABE6`.
Backend and driver binaries are unchanged.

The app-only upgrade completed on
2026-10-12 with MSI exit code 0. All 596 installed payload files matched the
manifest. NTPods driver hashes, valid signatures, Alternative A2DP 1.8.0.1, startup
entries and direct-daemon helper tasks were preserved. The app and daemon started
from Program Files, and the normal window rendered correctly. The AirPods were
disconnected and the microphone was idle, so no live audio check was performed.
Installation evidence is in `../../outputs/ntpods-0123-after.json`,
`../../outputs/ntpods-upgrade-0123-result.json` and
`../../outputs/ntpods-0123-after-install-state.jsonl`.

## 0.1.24 model compatibility preview

Rust and WinUI now share a catalog covering AirPods 1–4, Pro 1–3 and Max 1–2,
including their case and port variants. Capabilities require a matching model,
appropriate firmware and reported settings where applicable. Legacy ANC models
retain Off, Transparency and Noise Cancellation without Adaptive geometry.
Unknown models do not inherit Pro 3 microphone, EQ or sensor capabilities.
Pair selection, driver interface matching, battery keys, cached artwork and
pending commands now follow the selected paired identity.

The native UI comparison covered 25 simulated model/theme/width cases, including
dark/light at 640/316 DIP, legacy three-mode controls, Max single-battery artwork,
unknown devices and a disconnected Pro 1. No hardware commands were sent by those
previews. Evidence is in `../../outputs/model-compatibility-native.json` and the
matching images. The full Windows build passed native driver/decoder checks,
IPC and UI state tests, and installer preservation checks. Firmware follow-up
tests reject packed/internal version strings at capability boundaries and cover
metadata normalization to public versions.

The listening icons now clip their transparent bitmap inset to prevent a texture
edge from appearing during fractional movement. The isolated native build passed
dark/light, fractional-width and transition checks with the artwork intact. The
original reported line did not reproduce in the offline capture. The user then
confirmed that the line was gone in the final live preview, including while
dragging between modes.

Live preview checks on 2026-10-12 used the Intel AX200 and first-generation Pro
A2084. The device supplied firmware `6619472`, normalized to `6F21`. Windows
connection selected the correct AAP interface; the app reported the pair's name,
model, left/right batteries and case battery. The known case percentage remained
after removing the buds, with its freshness marked correctly. Listening checks
confirmed Off, Transparency and Noise Cancellation changes.
Automatic pause/resume worked, and manually paused playback stayed paused after
ear removal/reinsertion.

Press speed changed from 0 to 1 and back to 0, with both values verified by a fresh
control session. This firmware did not echo the writes or replay them after a
notification request; they remain unconfirmed until refreshed. The same silent
behavior made the old keepalive check falsely mark a working connection stale.
The check now runs only on eligible newer listening devices. Pro 1 movement on
service 14 timed out without accepted samples; it is an unresolved compatibility
issue, not a hardware pass. No high-quality microphone request was sent to Pro 1.

Switching back to Pro 3 selected A3064 on 9A348, restored Adaptive and the correct
pair's battery values, and left no pending settings or microphone stream. Evidence
is in `../../outputs/ntpods-0124-pro1-hardware.json`, the Pro 1 state/readback
captures, and `../../outputs/ntpods-0124-switch-pro3.jsonl`. Other models still need
their own hardware checks, including Max motion service discovery.

The preview runs from the workspace. The installed app remains 0.1.23; installed
NTPods drivers and Alternative A2DP were not replaced during these checks.
The final build passed 155 backend tests, UI state/gesture checks, 40 startup
preservation cases and 18 driver-preservation cases. Its package is
`windows/build-output/features-0.1.24/NTPods-experimental.msi`, SHA-256
`8040B87731395450D1452745E72D44DAEF01EAE1F29A6CCEF1957637913AA7BE`.
The final preview reconnected to Pro 3 on 9A348 with no stale-link or setting error,
an idle microphone and head tracking stopped.

## 0.1.25 compatibility and idle controls

The catalog now includes both AirPods 5 case variants and a conservative
Powerbeats Pro 2 profile. Max Crown/call controls and reported sleep detection
follow their own capability gates. AirPods 1/2 no longer expose optimized
charging, and Max does not expose the unverified earbud microphone-side selector.
Loud Sound Reduction has an independent, confirmed ATT read/write path under the
comfort submenu; it does not enable hearing assistance on Max. Failed ATT
transactions block further ATT writes until a fresh connection clears the queue.

The Pro 3 idle-control failure reproduced with `connected=true`, driver status
`Ok(2)`, and `link_stale=true`. Repeated writes of the unchanged ANC mode did not
receive replies. Those probes and periodic idle ANC nudges are removed for all
models. Quiet connections remain usable; a recovered driver status clears a
transport-derived stale flag without waiting for another device packet. Three
regression tests cover five minutes of quiet status, transport recovery and the
existing three-check Windows disconnect teardown. Capture recovery is unchanged.
The 0.1.25 preview stayed connected without stale controls through a two-minute
live check on Pro 3 A3064, firmware 9A348. The user then confirmed that listening
mode changes worked audibly. A one-step Adaptive strength write did not echo
(consistent with this firmware's existing silent-write behavior); its original
value was sent again afterward. Evidence is in `../../outputs/idle-controls-0125.json`.

Automatic pause is one line, with behavior in accessible help. The redundant top
device tab and separate Settings row are removed. Settings is in the device's
three-dot menu and remains accessible without a daemon connection; Back restores
focus to that menu button. Missing artwork
reuses the closest bundled images without changing identity or capabilities:
AirPods 5 uses AirPods 4 artwork, unknown/Powerbeats earbuds use Pro 2 artwork,
and unknown headphones use Max artwork. This preference is recorded in AGENTS.md.
Artwork brightness now follows availability separately from reading freshness.
Connected earbuds/headphones remain bright even when their cached percentages
age; an unavailable case is dim. Rings and percentages retain their own stale
presentation. Fresh nearby reports brighten only the reported component, and
the existing image fades remain on the images inside their opacity containers.

The full build passed 168 backend tests, 10 Rust IPC tests, cross-language IPC
fixtures including 87 malformed inputs, UI state/gesture/session tests, native
driver/decoder checks, and installer preservation tests. Native offline renders
covered seven model states in both themes at 640/316 DIP, plus wide/narrow shell
and Settings navigation. All 14 pause rows stayed 72 DIP tall with a single-line
label. Evidence is in `../../outputs/compatibility-0125-native.json`, its images,
and `../../outputs/build-0125-release.log`. These simulated layouts sent no commands
to the physical earbuds.

The final 0.1.25 preview ran from
`windows/build-output/features-0.1.25-final/dist/winui`; its backend matched the
packaged backend. The two-minute idle check passed and the user confirmed mode
changes. Dark/light native captures also verified the reported bright-case/dim-buds
state, Settings event routing, Back focus and Settings access with the daemon down.
The installer is
`windows/build-output/features-0.1.25-final/NTPods-experimental.msi`, SHA-256
`C7ED833D4132F4F94C51F220A5A719E0EE20AB1C7D5720633846A9D7B517EC8A`.

On 2026-10-12, the app-only upgrade from 0.1.23 completed with MSI exit
code 0; no restart was required. All 597 installed payload files match the build
manifest. NTPods AAP, ACX microphone and Alternative A2DP driver hashes are
unchanged, and both Start with Windows entries still point to Program Files.
The preview was shut down with the microphone idle. One UI and one backend now
run from `C:\Program Files\NTPods`. The installed backend reconnected to Pro 3
A3064, firmware 9A348, with `link_stale=false`, `mic_state=idle` and no settings
error. Evidence is in `../../outputs/ntpods-upgrade-0125-result.json`,
`../../outputs/ntpods-0125-after.json` and
`../../outputs/ntpods-0125-installed-state.json`.

New-model controls and Loud Sound Reduction still need physical-device checks.
Legacy double-tap actions remain hidden because their value encoding is
unverified. Max 2 and Powerbeats BLE battery layouts also remain unverified;
their proposed identifiers alone are not enough to enable decoding. Earlier
microphone, licensing and production-signing requirements remain unmet.

## 0.1.26 publication review and individual artwork availability

Code review found that fragmented UTF-8 commands could corrupt non-ASCII device
names. The daemon now assembles bounded byte input and decodes complete JSON
lines. Regression tests cover every byte split of accented, CJK and emoji names,
malformed UTF-8, and the existing input limit. The ACX capture circuit also checks
allocation and name-assignment failures before continuing initialization.

Battery reports now retain explicit component presence separately from cached
percentages. A reported absent bud dims while its partner remains connected;
aging a previously present bud's reading does not dim its artwork. Fresh nearby
charging reports remain bright. The nullable IPC field remains compatible with
older clients and reports. A new connection clears prior presence information.

The focused checks passed 171 daemon tests, 11 Rust IPC tests, cross-language
roundtrips and 91 malformed IPC inputs, UI/session/startup checks, native driver
and decoder checks, and installer shutdown/rollback tests. Sixteen isolated
native renders covered left absent, right absent, aged readings and nearby
charging in both themes at 640 and 316 DIP. Evidence is in
`../../outputs/battery-presence-0126-native.json` and its images. The render harness
sent no device commands. A physical one-bud retest remains pending.

Plain-text submenu headers previously inherited the native 14-DIP size, while
custom headers used 16 DIP. The shared header toggle now uses 16-DIP normal text
and inherits the expander's font family. Section captions and supporting text
retain their existing sizes; the change does not alter expanded controls.
Native before/after checks measured all ten header labels in both themes and
both widths: all 40 final labels use 16 DIP, weight 400 and Segoe UI Variable.
The isolated preview sent no commands. Evidence is in
`../../outputs/header-typography-0126-before.json`,
`../../outputs/header-typography-0126-after.json` and their images.

The publication review updated the README, design notes and installer wording.
The microphone license remained unresolved, with no new grant or upstream
clarification. Binary publishing was disabled for the initial source push.

## Native playback driver and bitrate controls

The playback driver binds the paired AirPods Pro 1 and Pro 3 AudioSink instances.
It replaces the root virtual speaker used by the earlier playback experiment.
AAC and AAC-ELD share one output, with separate Best effort and Manual bitrate
preferences. Other AirPods models keep their current playback provider.

AAC encoder tests cover every 1 kbps step from 96 through 512 with full-scale
noise, impulses and transients. Seven rates also pass decoded pitch and stereo
separation checks. AAC-ELD checks cover 128 through 256 kbps headers and decoded
samples at five rates. Both encoders run in separate processes; a suspended-worker
test verifies that shutdown kills the worker and releases blocked pipes.

Installer tests cover exact-device binding, original-package backups, failed
exports, failed restoration, upgrade rollback and restoration after an absent
pair returns. Read-only checks found compatible drivers for both local pairs;
no new driver was installed. Sixteen native UI renders cover codec and bitrate
controls in light/dark themes at desktop and narrow widths.

The replacement driver still needs physical installation, playback, microphone
coexistence, volume/mute, sleep/resume and uninstall testing. Lower-rate AAC-ELD
acceptance by the AirPods remains unverified. Prior microphone results used
Alternative A2DP and do not validate this driver.

## Production release requirements

Production redistribution is held by the ACX sample licensing discrepancy described
in `THIRD_PARTY.md`. Author clarification or a compatible replacement is required;
changing a license label does not resolve it. Normal deployment also requires all three
driver packages to pass Microsoft's signing process and load with Secure Boot and
Memory Integrity enabled. The local test-signing installer is not that release path.
