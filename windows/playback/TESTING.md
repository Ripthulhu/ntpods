# Playback test

Use a Windows 11 x64 test PC with paired AirPods and Test Mode already enabled.
Test Mode weakens driver-signature checks; do not enable it on a work or everyday
PC for this experiment. AAC and AAC-ELD playback have not passed hardware testing.

Download **NTPods.msi** from [releases](https://github.com/Ripthulhu/ntpods/releases)
and install it normally. It includes both codecs and updates both NTPods drivers.
Restart if setup requests it. The Windows ZIP contains the same files for manual
installation; see [the installation guide](../README.md).

## Check each codec

1. Connect your AirPods and confirm normal music playback works.
2. Stop recording. In **Playback and microphone**, choose **AAC**.
3. In your player or Windows Volume mixer, select **Speakers (NTPods Experimental)**
   for that app. Start at low volume with shared-mode playback.
4. Check distinct left/right tones, speech pitch, sustained playback, pause/resume,
   volume and mute. Record with **Microphone (NTPods)** while stereo plays, then
   stop recording and check both earbuds again.
5. Select **AAC-ELD** and repeat. Check switching in both directions.
6. Select **Windows**, choose the usual AirPods output in the player and confirm
   ordinary playback works again.

The app remembers the selected mode. The experimental endpoint cannot become
the Windows default, so route each playback app to it. Codec changes are blocked
while the microphone is active or recovering.

Also check disconnect/reconnect, case closure, sleep/resume and app exit. After a
native playback failure, normal audio services should return; use Retry to attempt
the selected codec again. After a crash, reopen NTPods to restore the saved audio
services. Keep `playback-restore.json` if restoration fails; it is the recovery
record, not a cache to clear.

Record the codec, AirPods model and firmware, Windows version, Bluetooth adapter
and the step that failed. Backend logs are under `%LOCALAPPDATA%\NTPods`; remove
personal device identifiers before sharing them. Audio payloads are not logged.
The existing A2DP driver package is retained throughout installation and testing.

For source builds and automated checks, see [the playback README](README.md).
