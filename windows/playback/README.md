# AirPods playback

NTPods replaces the playback driver on the paired AirPods AudioSink service.
AAC and AAC-ELD use the same AirPods output, which can be the Windows default.
There is no extra virtual playback device or per-app routing requirement.
The initial driver replacement may make Windows create a new endpoint ID;
subsequent codec changes keep that endpoint.

The first playback driver targets AirPods Pro 1 and Pro 3. Other models retain
their existing playback driver. Both codecs are lossy 48 kHz stereo and remain
experimental: playback, microphone coexistence and driver recovery still need
hardware testing.

## Use it

Install the MSI or Windows ZIP from [releases](https://github.com/Ripthulhu/ntpods/releases).
Both contain the app, matching drivers and both encoders. Test Mode is required;
use a dedicated test PC. See [TESTING.md](TESTING.md) for the test procedure.

Keep NTPods running while playing audio. Enable Start with Windows to start it
after login. Select the AirPods in Windows Sound as usual, then choose AAC or
AAC-ELD under **Playback and microphone**. AAC is the initial choice. Stop
recording before changing codecs or bitrate. Use shared-mode playback; exclusive
mode bypasses Windows software volume and mute.

Each codec remembers its own bitrate setting:

| Setting | Behavior |
| --- | --- |
| Best effort | Highest implemented rate within the peer's advertised limit and packet budget. |
| Manual | Chosen kbps: AAC 96–512; AAC-ELD 128–256. Unsupported rates report an error. |

Best effort does not monitor radio quality or promise a fixed delivered rate.
The encoders use variable-sized frames; kbps sets a limit, not a guarantee that
every second contains that many bits. Codec and bitrate changes restart the
Bluetooth stream without removing the Windows output.

A failed stream supplies silence and offers Retry. Closing or crashing the
backend also stops playback. It cannot fall back to Microsoft's driver while
NTPods owns the AudioSink. Keep supported pairs connected during uninstall so
setup can restore their previous playback driver. Its package is retained and
backed up during installation. Restart Windows if setup requests it.

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
a per-device PCM bridge. The backend checks the bridge's Bluetooth address before
opening AVDTP. The AAP driver's shared handle keeps playback alive across AAP
control-channel retries. Each bridge accepts one reader and holds at most 100 ms
of PCM; it clears queued audio when that reader closes.

AAC uses 1024-sample blocks with LATM framing and the 90 kHz RTP clock specified
by [RFC 6416](https://www.rfc-editor.org/rfc/rfc6416.html). AAC-ELD uses 480-sample
blocks, three access units per packet, and Apple vendor codec `004C/8001`.
Its framing follows [USBPods' macOS capture research](https://github.com/wasdwasd0105/USBPods-Pico2W/blob/6286ab1618ec1143d828b1fc377d8e22c449e567/aac-eld-apple.md).
Signaling, media, AAP and ATT use separate channels. Audio payloads are not logged.

Microphone recovery restarts the AVDTP stream while this driver owns playback.
It must never disable the Windows AudioSink service, which would remove the
playback endpoint. Actual recovery timing and stereo coexistence remain hardware
test requirements.

Automated checks cover packet validation, cancellation, per-device PCM isolation,
buffer limits, bitrate settings, and encoder pitch/stereo output. AAC round trips
use FFmpeg. AAC-ELD round trips use FDK's decoder because the pinned FFmpeg cannot
decode low-delay SBR. Independent AAC-ELD decoding and acceptance by the AirPods
remain unverified.
