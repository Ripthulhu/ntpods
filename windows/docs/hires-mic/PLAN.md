# AirPods microphone on Windows

NTPods exposes the AirPods AAC-ELD uplink as a Windows microphone. Recording apps
use the ACX capture endpoint; the daemon receives and decodes the Bluetooth audio.
The protocol follows [LibrePods PR #655](https://github.com/librepods-org/librepods/pull/655).
Supported models and hardware limits are listed in
[model support](../../../docs/WINDOWS-MODEL-SUPPORT.md).

```text
AirPods → AAP/L2CAP → NTPodsAAP → ntpodsd → NTPodsMicACX → recording app
                                   AAC-ELD decoding       48 kHz mono PCM16
```

## Audio path

The daemon sends AAP `START_AUDIO` and `STOP_AUDIO` commands and receives opcode
`0x58` packets. [`aap.rs`](../../daemon/src/aap.rs) validates access
units before [`eld.rs`](../../daemon/src/eld.rs) passes them to FFmpeg through
[`eld_shim.c`](../../daemon/src/eld_shim.c).

The decoder uses AudioSpecificConfig `F8 E6 30 00` and expects 480-sample mono
frames. The shim applies the upstream 64 kHz source-clock interpretation, then
resamples to 48 kHz PCM16 with make-up gain and a soft limiter. The ASC itself
signals 48 kHz; the 64 kHz interpretation remains a calibration assumption.
Pitch and long-session drift require recordings, not just successful decoding.

[`micpipe.rs`](../../daemon/src/micpipe.rs) writes PCM to `\\.\NTPodsMic`. The
[ACX driver](../../drivers/mic/README.md) buffers those samples and supplies them
to Windows capture clients. It emits silence on underrun and limits queued audio
to prevent drift from accumulating into an ever-growing delay.

## Start, stop and recovery

[`mic_lifecycle.rs`](../../daemon/src/mic_lifecycle.rs) separates user intent from
recording activity and Bluetooth recovery:

- Automatic starts when Windows has a capture client and stops after the chosen
  delay, from 0 to 3,600 seconds after capture ends.
- Keep active requests the microphone while the device can capture.
- Manual uses the app or keyboard shortcut to start and stop it directly.

Wear state also gates capture. A resumed uplink gets a fresh decoder and resampler
after a packet gap of at least two seconds, preventing stale codec history from
producing static. Stop and recovery flush queued PCM.

Changing microphone mode can disturb playback. With NTPods playback, recovery
restarts AVDTP without disabling the AudioSink service or removing the output.
The Windows playback path uses its separate recovery logic. See
[microphone protocol](../../../docs/WINDOWS-MIC-PROTOCOL-RESEARCH.md) and
[playback](../../playback/README.md) before changing either path.

## Build and validation

Use the complete [Windows build](../../../docs/BUILD-WINDOWS.md). It builds the
matching daemon and drivers and provisions access through the installer. The
drivers require Test Mode; the retired single-driver installers are unsupported.

Automated checks cover malformed audio, decoder reset, lifecycle transitions,
PCM buffering and driver request validation. Hardware checks must also cover
earbud removal/reinsertion, microphone stop, stereo recovery, pitch and long
recordings. Current results and gaps are in
[WINDOWS-VALIDATION.md](../../../docs/WINDOWS-VALIDATION.md).
