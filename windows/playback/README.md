# AirPods playback

NTPods replaces the playback driver on the paired AirPods AudioSink service.
AAC and AAC-ELD use the same AirPods output, which can be the Windows default.
There is no extra virtual playback device or per-app routing requirement.
The initial driver replacement may make Windows create a new endpoint ID;
subsequent codec changes keep that endpoint.

The playback driver targets AirPods 1–4, Pro 1–3 and the original Max, using the
exact IDs in the [model list](../../docs/WINDOWS-MODEL-SUPPORT.md#native-playback).
Pro 1 and Pro 3 have passed short hardware checks; the other listed models are
available to test. Both codecs are lossy 48 kHz stereo and remain experimental.

## Use it

Install the MSI or Windows ZIP from [releases](https://github.com/Ripthulhu/ntpods/releases).
Both contain the app, matching drivers and both encoders. Test Mode is required;
use a dedicated test PC. See [TESTING.md](TESTING.md) for the test procedure.

Keep NTPods running while playing audio. Enable Start with Windows to start it
after login. Select the AirPods in Windows Sound as usual, then choose AAC or
AAC-ELD under **Playback and microphone**. AAC-ELD is the initial choice;
upgrades preserve your saved codec. Stop recording before changing codecs or
bitrate. Use shared-mode playback for testing.
Volume and mute use Windows' AVRCP service to control the earbuds' gain; the
[volume bridge](../../docs/WINDOWS-AVRCP-VOLUME.md) has partial Pro 3 hardware coverage.

If the pair does not advertise a supported AAC-ELD profile, NTPods uses AAC with
its saved bitrate setting. The codec menu keeps your preference; its tooltip
shows the codec being used. Bitrate controls follow the active or attempted
codec, so a rejected AAC bitrate can be corrected even when AAC-ELD is preferred.
Stream failures and invalid manual bitrates still report errors.

Each codec remembers its own bitrate setting:

| Setting | Behavior |
| --- | --- |
| Best effort | Highest implemented rate within the peer's advertised limit and packet budget. |
| Manual | Chosen kbps: AAC 96–512; AAC-ELD 128–256. Unsupported rates report an error. |
| Variable (experimental) | AAC-ELD only, with a 256 kbps peak limit. Requires the peer and packet budget to support that limit. |

Best effort does not adapt to radio conditions. AAC targets the selected rate
within its frame-size limit. AAC-ELD Best effort and Manual use constant bitrate;
individual frames can vary by one byte. Variable mode changes bitrate with the
content and has passed short Pro 3 checks. Codec and bitrate changes restart
the Bluetooth stream without removing the Windows output.

A failed stream supplies silence and offers Retry. Closing or crashing the
backend also stops playback. It cannot fall back to Microsoft's driver while
NTPods owns the AudioSink. Keep supported pairs connected during uninstall so
setup can restore their previous playback driver. Its package is retained and
backed up during installation. Pairs first connected after installation may bind
automatically; without a saved prior binding, uninstall restores Microsoft's
Bluetooth audio driver. Restart Windows if setup requests it.

To restore playback alone, connect the AirPods and run `restore-playback.ps1`
from the installed folder or the extracted Windows ZIP in an administrator
PowerShell. This keeps the app and microphone. The same script finishes a
pending restore if the AirPods were absent during uninstall. Use the extracted
ZIP after MSI removal, since the installed helper files will be gone.

## Build

Use the [Windows build prerequisites](../../docs/BUILD-WINDOWS.md), then run:

```powershell
pwsh -File windows/build.ps1
```

The build tests both codecs and creates `windows/build-output/NTPods-experimental.msi`.
It does not install drivers or change Windows settings. Separate worker processes
use FFmpeg for AAC-LC and [FDK](encoder/README.md) for AAC-ELD, so an encoder stall
can be cancelled without blocking the backend. The package includes
FDK's notice and complete pinned source archive.

## Transport

The ACX playback driver binds Apple's model-qualified AudioSink IDs and exposes
a per-device PCM bridge. The backend checks the bridge's Bluetooth address and a
current explicit Connect, Repair, audio rebuild or pairing request before opening
AVDTP. A nearby advertisement or control-only Windows link cannot claim playback.
After startup or a completed Bluetooth-link loss, use Connect to resume custom
playback; the driver currently provides no verified peer-initiated audio-return
signal. Windows' own profile reconnection is independent of this policy.
Ear detection pauses or resumes local media only while the selected audio output
is ready; passive control reports cannot pause or resume PC media.
The AAP driver's shared handle keeps playback alive across AAP control-channel
retries. Each bridge accepts one reader and holds at most 100 ms
of PCM; it clears queued audio when that reader closes. PCM overruns and missed
processing deadlines retry the stream after one second while keeping existing
Windows audio clients connected. Failed cleanup stops playback until an explicit
retry or reconnection.

The Windows endpoint prefers 48 kHz stereo Float32 and also accepts PCM16.
The driver retains float precision through the PCM bridge. AAC-LC accepts Float32;
AAC-ELD converts directly to 32-bit fixed-point PCM in its matching FDK worker.
The backend still supports
older PCM16 playback drivers; those cannot preserve very quiet float signals.

AAC uses 1024-sample blocks with LATM framing and the 90 kHz RTP clock specified
by [RFC 6416](https://www.rfc-editor.org/rfc/rfc6416.html). AAC-ELD uses 480-sample
blocks without SBR, one access unit per packet, a 48 kHz sample-count RTP clock,
and Apple vendor codec `004C/8001`. AAC-ELD uses a 20 ms PCM source reserve;
AAC uses 30 ms, including when selected as a fallback.
The AAC-ELD encoder and transport follow the
[USBPods fork tested on Pro 3](https://github.com/han-um/USBPods-Pico2W-2-way-audio-implement/blob/d18d083bc52de3143d8d922c4abaae865cc0dc28/src/btstack/btstack_avdtp_source.c),
with bitrate capped at 256 kbps. RTP uses the tested source identifier `0x11223344`.
Millisecond timestamps remain
[diagnostic overrides](../../docs/WINDOWS-AUDIO-LATENCY.md#packet-and-source-reserve-overrides):
both a 1 kHz sample counter and monotonic milliseconds with SSRC zero caused
Pro 3 playback to stop after about three seconds.
Signaling, media, AAP and ATT use separate channels. Audio payloads are not logged.

Microphone recovery restarts the AVDTP stream while this driver owns playback.
It must never disable the Windows AudioSink service, which would remove the
playback endpoint.

When offered by the earbuds, NTPods negotiates AVDTP Delay Reporting and records
their reported delay. This is separate from measured Windows-to-sound latency.
Receiver reports stay in IPC and diagnostics. The ACX hardware-latency callback
returns zero: publishing the remote delay there can prevent Windows from
initializing playback. It is not an end-to-end latency estimate.
The [automatic receiver-buffer policy](../../docs/WINDOWS-AUDIO-LATENCY.md#automatic-receiver-buffer-policy)
requests 60 ms for gaming-capable AAC-ELD devices and Pro 3, with a coordinated host-clock adjustment
and media flush timeout. It restores a chosen 150 ms target for microphone use,
pause and playback changes. Other models without capability evidence retain their receiver settings.
Pro 3 listening checks with 0.2.48 and the default 48 kHz timestamp format
confirmed clean AAC-ELD playback through pause/resume and microphone
capture/recovery, with unchanged volume. See
[validation](../../docs/WINDOWS-VALIDATION.md#playback-latency-investigation)
for conditions and limits; other models and acoustic latency were not retested.
The [packet and source-reserve overrides](../../docs/WINDOWS-AUDIO-LATENCY.md#packet-and-source-reserve-overrides)
allow comparison with two-frame packets and a 50 ms reserve for a test session.
They also provide timestamp and source-identifier comparisons without changing
codec selection or volume.

Automated checks cover packet validation, cancellation, per-device PCM isolation,
buffer limits, bitrate settings, and encoder pitch/stereo output. AAC round trips
use FFmpeg; AAC-ELD round trips use FDK's own decoder.

Recorded Pro 1 and Pro 3 checks cover both codecs and 70-second AAC-ELD playback.
Pro 3 also passed short stereo/microphone coexistence checks. Long sessions,
repeated start/stop, sleep/resume, uninstall recovery and other models remain
untested. See the [validation results](../../docs/WINDOWS-VALIDATION.md).
