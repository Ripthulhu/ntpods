# Pico dongle integration

NTPods controls the dongle through its vendor HID collection. Windows' USB audio
driver owns playback and capture. The app does not need CDC, a direct AirPods
Bluetooth connection, or another media/volume relay for this path. Standalone
audio, stems and reconnect remain firmware features.

## Connection lifecycle

Firmware 0.1.44 boots as control-only `CAFE:4005`. It exposes `CAFE:4015` with USB
audio after the selected AirPods' audio-profile connection is established, unless
Bluetooth phone input excludes USB audio. Both personalities retain the same
serial and vendor usage `FF31:0001`. NTPods discovers that collection and validates
the 64-byte `APDG` status instead of relying on interface numbers.

A personality change detaches USB briefly. The daemon reopens HID, starts a new
host session, retires old commands and reloads settings. A board remains attached
when USB headphones are absent. The app distinguishes the board, AirPods link,
control readiness and USB audio availability. None proves that Windows selected
the dongle output or that the board owns the AirPods audio route.

With several boards attached, choose one in the dongle card. Selection follows
its serial across re-enumeration. If it is unplugged, another board is not chosen
silently. The main Connect button reconnects the saved AirPods through the
selected dongle. Pair a new set from the dongle card.

Open **AirPods Dongle → Connect automatically** to let that board connect to its
saved AirPods when they come into range. The switch reads the preference from
the board and saves it across power cycles. To change it, stop head tracking
and pause playback and recording. With the switch off, a button press or
**Connect** still requests the AirPods, and a connection initiated by the
AirPods is still accepted.

With the multipoint preview, the dongle opens the saved AirPods' Bluetooth
audio connection in standby so Windows can offer it as an output. The phone
can interrupt PC music. Once the AirPods report that phone audio has ended,
the dongle can take back the route
if Windows is still sending audible samples. A paused player stays paused.
Earlier firmware with the same switch can reclaim the AirPods as soon as it
connects and may need a button press to return from an interruption.

## Settings and confirmation

Settings read automatically. Transient initial reads retry with bounded backoff;
writes never replay after timeout or session loss. Live editors retain one
in-flight write and the latest complete choice, preserving untouched fields.
Changes to clinical hearing modes and profiles use the existing firmware workflow
lease, confirmed baselines and restoration checks.

Local command acceptance remains separate from fresh AirPods readback. Firmware
controls model/value support; unknown settings do not become confirmed defaults.
Last-known battery values retain their independent observation ages. A retained
case percentage never grants charging, presence or audio ownership.

Output codec/profile and Game Mode use firmware typed features. Codec and bitrate
changes require idle audio and renegotiate the output connection. Game Mode can
change live, starts off at firmware boot and has separate pending, sent and
confirmed states. Firmware performs timing adjustments; the app adds no gain or
clock-control loop.

## Spatial audio

Open **Spatial audio** in the device settings for a selected AirPods Pro 3
pair. Start, calibration and ear profile comparison all target the selected
dongle. The renderer keeps its per-bud calibration and re-centers when media
resumes. It requests up to 100 Hz motion, with the firmware cap of 30 Hz
during Game Mode.

Set Steam Streaming Speakers to 5.1 or 7.1 and route the game or player there.
Keep the dongle microphone idle. Start and stop playback in NTPods.
The Spatial audio page guides calibration and previews the bundled ear profiles.
Pause other audio before comparing profiles. Use **Windows sound settings**
to open the system controls for your surround input.

The **Listening room** shows your head and the surround speakers in a 3D studio.
Drag the room to orbit, scroll to zoom, or use **Reset view** to return to
the starting camera. The camera moves independently of the sound. The
speakers respond to the peak level
of each Windows input channel. A mixed game soundtrack can light several
markers at once. During a motion interruption, the renderer keeps playing
and watches the selected dongle for fresh reports. The motion-recovery
firmware can restart sensor discovery while the AirPods remain connected.
If the audio connection closes, the room waits for the same AirPods and
dongle. Playback resumes when their Windows audio output returns and the
microphone is idle. Choose **Stop head tracking** to cancel the wait.

Drag a speaker to reposition it, or select it
to open **Speaker placement**. Adjust soundstage width from 50% to 300%,
set a speaker's azimuth and elevation in degrees, or trim its level by up
to 24 dB. The low-frequency effects channel has a level control. Use the
speaker picker for overlapping markers or keyboard adjustments. Changes
take effect during playback. NTPods saves each Windows speaker layout
separately (5.1 rear, 5.1 side and 7.1), including its channel levels.
At 100%, speakers keep their configured bearings. At 300%, the default front
speakers move from 30° to 60° either side of center. The center stays ahead.
**Reset this layout** restores the current layout to its factory placement.
If WebView2 or WebGL is unavailable, the app shows a native top-down
diagram and keeps the placement controls available.

## CLI coverage

The daemon already handles pairing, reconnect, listening modes, stem controls,
Game Mode, Custom Transparency, Hearing Aid and Headphone Accommodations.
These controls use the existing HID session and device readback.

EQ, loud-sound reduction, explicit disconnect
and extended catalog inspection are available through the firmware CLI.
Their desktop controls require firmware capability checks and confirmed
readback. Keep these actions in the CLI until the daemon has those mappings.
[Firmware CLI guide](https://github.com/Ripthulhu/airpods-dongle/blob/main/docs/CLI.md)

## Firmware gaps and acceptance

A single button press requests audio ownership. The CLI can confirm ownership
through the routing transaction on supporting firmware. The older reconnect
command confirms acceptance. A double press toggles Game Mode.

S16 and IEEE float32 stereo at 48 kHz are supported USB inputs; the microphone is
S16. Descriptor support, an audio file's format and output codec do not prove the
selected USB alternate. Typed feature 11 offers a bounded diagnostic readback,
not a reason to continuously poll its 512-byte trace. The app does not change the
Windows default output or its volume merely by discovering the board.

Offline tests cover discovery/selection, session fencing, timeout and retry
handling, state/command IPC and presentation. They do not establish listening
quality, latency, current draw, hearing DSP effectiveness or volume stability on
0.1.44. Those checks remain separate from builds. In particular, the premature
Android maximum-volume cue and the earlier runtime BOOTSEL busy result remain
firmware investigations; no app gain workaround is applied.
