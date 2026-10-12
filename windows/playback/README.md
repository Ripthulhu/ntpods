# Experimental playback

This development build adds `Speakers (NTPods Experimental)`, a 48 kHz stereo
playback endpoint. It sends standard AAC or Apple AAC-ELD through its own AVDTP
connection, without Alternative A2DP. The normal build still uses Windows' existing
playback driver.

Playback on AirPods, microphone coexistence and recovery are not yet validated.
Both codecs are lossy. Standard AAC uses the FFmpeg library already shipped with
NTPods. AAC-ELD needs a separate encoder whose redistribution licensing remains
unresolved; see [the encoder README](https://github.com/Ripthulhu/ntpods/blob/main/windows/playback/encoder/README.md). The normal MSI and Windows
ZIP keep the existing playback path.

Download `NTPods-Native-AAC.zip` from
[releases](https://github.com/Ripthulhu/ntpods/releases) for the AAC test build.
It includes both experimental drivers and requires a working NTPods installation
on a test PC. Start with [TESTING.md](TESTING.md) to install and later restore the
original drivers. AAC-ELD binaries are excluded from this download.

## Build

Use the [Windows build prerequisites](https://github.com/Ripthulhu/ntpods/blob/main/docs/BUILD-WINDOWS.md). From the
repository root:

```powershell
pwsh -File windows/playback/build.ps1
```

The script tests and builds both optional drivers, the AAC encoder and the backend.
Output goes to `windows/build-output/native-playback`, separate from the normal
build. It does not sign or install drivers or change Windows settings.

From a clean commit, run `pwsh -File windows/playback/package.ps1` after the build
to create the test ZIP. The packager checks that the binaries match the build
manifest and includes unsigned catalogs, installer helpers and checksums.

To also build AAC-ELD, install the Visual Studio 2022 CMake tools and Python 3,
then add `-IncludeEld`. That build goes to `windows/build-output/experimental-playback`
and includes the separate FDK encoder. The default AAC build does not need FDK.

| Folder | Contents |
| --- | --- |
| `runtime` | Backend and required DLLs; optional AAC-ELD worker |
| `driver` | Experimental Bluetooth driver, unsigned |
| `driver-mic-acx` | Experimental capture/render driver, unsigned |

The drivers require development signing and Test Mode, as described in the
main [README](https://github.com/Ripthulhu/ntpods#ntpods). Use the [test installation and rollback procedure](TESTING.md)
to keep the previous working driver packages. Installing just the backend does
not enable this feature.

## Try it

After installing the matching development drivers:

1. Connect the AirPods with normal playback working, and stop any recording.
2. Quit NTPods, including its tray process, so the backend releases the driver.
3. Run `runtime/ntpodsd.exe --experimental-playback=aac` from the output folder.
   Use `--experimental-playback=eld` for AAC-ELD when built with `-IncludeEld`.
4. Open NTPods again for controls. In the player or Windows Volume mixer, select
   `Speakers (NTPods Experimental)` for that app and start at low volume.

The endpoint cannot become the Windows default. While experimental playback is
active, NTPods volume, mute and Conversation Awareness control that endpoint.
Use shared-mode playback; exclusive mode bypasses Windows software volume and
mute. Starting the backend without the flag leaves the normal playback driver
in control. Each flagged process attempts one playback session; restarting it
is required after a failure or disconnect.

Before switching audio services, the backend checks the render interface,
Bluetooth driver and encoder. It saves the enabled services, then releases the
AirPods' normal audio services while the experiment runs. An error or normal
exit closes the AVDTP channels and restores those services. A newer Disconnect
request stays disconnected.

After a crash, launching this updated backend, with or without the flag, restores
the saved services from
`%LOCALAPPDATA%/NTPods/playback-restore.json`. The file is removed only after
successful restoration or an explicit Disconnect. Do not delete it to work
around a restoration error. Older installed backends do not perform this recovery.
Details are in the normal backend log. Windows'
Bluetooth service operations have no cancellation API and can delay cleanup.

To return to normal playback, quit the experimental backend, start the installed
app normally, and select the AirPods' normal output in the player. The render
endpoint remains present until the original capture-only driver is restored.

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
