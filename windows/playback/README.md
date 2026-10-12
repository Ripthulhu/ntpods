# Experimental playback

NTPods includes Windows, AAC and AAC-ELD playback options under **Playback and
microphone**. Windows is the default. AAC and AAC-ELD send 48 kHz stereo through
NTPods' own AVDTP connection, without Alternative A2DP. Both codecs are lossy.
Playback on AirPods, microphone coexistence and recovery still need hardware testing.

Install the MSI or Windows ZIP from [releases](https://github.com/Ripthulhu/ntpods/releases).
Both contain the app, matching drivers and both encoders. Start with
[TESTING.md](TESTING.md) for the laptop test procedure.

## Build

Use the [Windows build prerequisites](../../docs/BUILD-WINDOWS.md), including
Visual Studio 2022 CMake tools and Python 3. From the repository root:

```powershell
pwsh -File windows/build.ps1
```

The normal build tests both codecs and creates `windows/build-output/NTPods-experimental.msi`.
It does not install drivers or change Windows settings. FFmpeg supplies AAC-LC;
a separate [FDK worker](encoder/README.md) supplies AAC-ELD. The package includes
FDK's notice and complete pinned source archive.

## Use it

Stop recording before changing playback. Choose AAC or AAC-ELD in NTPods, then
select **Speakers (NTPods Experimental)** in your player or Windows Volume mixer.
The endpoint cannot become the Windows default; choose it for each playback app.
Use shared mode, since exclusive mode bypasses Windows software volume and mute.
NTPods volume, mute and Conversation Awareness control the experimental endpoint
while native playback is active.

The app remembers your choice. To return to normal playback, select Windows in
NTPods and select the usual AirPods output in your player. Changing codecs stops
the previous session before starting another. A failed native session restores
normal audio services and shows an error with Retry.

Before releasing normal AirPods audio, the backend checks the render interface,
Bluetooth driver and encoder, then saves the enabled services. After a crash,
the updated backend restores that saved state on its next launch from
`%LOCALAPPDATA%/NTPods/playback-restore.json`. Do not delete this file to bypass a
restoration error. A newer Disconnect request stays disconnected. Windows'
Bluetooth service operations have no cancellation API and can delay cleanup.

## Transport and checks

Standard AAC uses 1024-sample blocks, 48 kHz stereo and a 256 kbps codec setting.
The encoder reserves space for LATM framing within that rate. Each frame carries
its stream configuration; oversized frames are fragmented at the negotiated MTU.
RTP timestamps use the 90 kHz clock specified by
[RFC 6416](https://www.rfc-editor.org/rfc/rfc6416.html).

AAC-ELD uses 480-sample blocks. Three frames go into each media packet, using the
Apple vendor codec `004C/8001`, 48 kHz stereo and a 256 kbps peak limit. Framing follows
[USBPods' macOS capture research](https://github.com/wasdwasd0105/USBPods-Pico2W/blob/6286ab1618ec1143d828b1fc377d8e22c449e567/aac-eld-apple.md).
AVDTP signaling and media use separate channels; AAP and ATT stay separate.

The PCM bridge holds at most 100 ms. The backend waits for complete frames and
fails back to normal audio if it cannot keep pace. It does not log PCM or AAC
payloads. A microphone recovery request tries AVDTP SUSPEND/START with a fresh
encoder while this path owns playback; failure ends the experiment and restores
the normal services. That sequence still needs an AirPods recording test.

Automated checks cover packet validation, channel cancellation/cleanup, PCM
buffer boundaries, source clock preservation and encoder pitch/stereo output.
AAC tests encode and decode stereo tones, noise and impulses through FFmpeg.
The AAC-ELD round trip uses FDK's own decoder because the pinned FFmpeg cannot
decode low-delay SBR. Independent AAC-ELD decoding remains a validation gap.

Hardware checks must cover endpoint enumeration, left/right playback, sustained
playback with microphone capture, microphone stop/reinsertion, app exit/crash,
Bluetooth disconnect and sleep/resume. Verify normal A2DP restoration after each
failure, then run Driver Verifier on the test machine.
