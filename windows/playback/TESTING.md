# Playback test

Use a Windows 11 x64 test PC with paired AirPods Pro 1 or Pro 3 and Test Mode
already enabled. Test Mode weakens driver-signature checks; do not enable it on
a work or everyday PC for this experiment. Short Pro 3 playback checks have
passed; full hardware validation is still pending. See the
[validation record](../../docs/WINDOWS-VALIDATION.md#native-playback-driver-and-bitrate-controls).

Download **NTPods.msi** from [releases](https://github.com/Ripthulhu/ntpods/releases)
and install with drivers selected. Enable Start with Windows or open NTPods after
login; playback requires its backend. Restart if requested. The ZIP includes the
same files for [manual installation](../README.md).

## Check each codec

1. Connect the AirPods and open NTPods. Select the AirPods output in Windows Sound.
   There should be one playback output for the pair, with no NTPods virtual speaker.
   The first driver replacement may require selecting the output again.
2. Under **Playback and microphone**, choose **AAC** with **Best effort**. Check
   system sounds and music at low volume, then distinct left/right tones, speech
   pitch, sustained playback, pause/resume, volume and mute.
3. Choose **Manual**, enter a bitrate and Apply. Test 128, 256 and 320 kbps where
   the peer supports them. Confirm the settings survive restarting NTPods.
4. Select **AAC-ELD** and repeat with Best effort and Manual 128, 192 and 256 kbps.
   Check that each codec keeps its own bitrate choice.
5. Switch codecs in both directions. Windows should retain the same output and
   default selection. No per-app output change should be needed.
6. Record with **Microphone (NTPods)** while stereo plays. Stop recording and check
   both earbuds. Also test removing/reinserting buds during recording.
7. Keep supported pairs connected, uninstall and confirm playback returns through the previous driver.
   Reconnect or restart if requested, then test installing again.

Also check case closure, Bluetooth disconnect, sleep/resume, AAP control reconnect,
backend exit/crash, and long playback. Codec/bitrate changes must be blocked while
the microphone is active or recovering. A failed stream should offer Retry;
restart NTPods after a crash. The replacement driver cannot play while its backend
is stopped. Driver Verifier and clean install/upgrade/uninstall remain required.

Record the codec, bitrate setting, AirPods model/firmware, Windows version,
Bluetooth adapter and failed step. Logs are under `%LOCALAPPDATA%\NTPods`; remove
personal device identifiers before sharing. Audio payloads are not logged.

Installation saves previous playback bindings under
`%ProgramData%\NTPods\audio-original`; failed driver transactions keep recovery
data under `%ProgramData%\NTPods\driver-rollback`. Do not delete either to bypass a
restoration error. The previous driver's package is never removed by installation.
If a pair was absent during uninstall, reconnect it and run `restore-playback.ps1`
from the extracted Windows ZIP in an administrator PowerShell to finish restoring
its saved provider. The script also restores playback without uninstalling the app.
