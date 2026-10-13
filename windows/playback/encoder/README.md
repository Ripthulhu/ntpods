# AAC-ELD encoder

This worker encodes 48 kHz stereo PCM for the Apple AAC-ELD playback experiment.
The normal build includes the worker and its separate FDK library in the MSI and ZIP.

## Build and check

Run from the repository root with Visual Studio 2022 C++ and CMake tools:

```powershell
./windows/playback/encoder/build.ps1
python windows/playback/encoder/test.py
```

The test encodes separate 440 Hz and 880 Hz tones, decodes through FDK, and checks
format, pitch, channel separation and malformed input. It does not play audio or
open Bluetooth. This round trip uses the encoder library's own decoder.

Build output and downloaded source stay in ignored `build` and `vendor` folders.
The executable and `fdk-aac.dll` are in `build/Release`.

## Pipe format

Input is signed 16-bit little-endian PCM, left then right, at 48,000 Hz.
Write complete blocks of 480 stereo frames (1,920 bytes). A partial final block
is an error. Closing stdin flushes the encoder and closes stdout.

Output starts with this header:

| Field | Encoding |
| --- | --- |
| Magic and version | Eight ASCII bytes: `NPELD001` |
| Samples per channel per frame | Little-endian uint16: 480 |
| AudioSpecificConfig length | Little-endian uint16: 1–64 |
| AudioSpecificConfig | That many bytes |

Each following record contains a little-endian uint16 length and one raw AAC-ELD
access unit. Lengths are 1–321 bytes. EOF ends the stream; there is no zero-length
record. Errors go to stderr and return a nonzero exit code. The caller must check
the exit code before treating EOF as a normal stop.

The encoder uses AOT 39 and 480-sample frames, with SBR disabled.
`--bitrate <bits-per-second>` accepts 128000–256000 in 1000-bit steps; omitted, it
uses 256000. FDK's CBR frames can vary by one byte. Each access unit is limited to
`ceil(bitrate / 800) + 1` bytes, at most 321 bytes at 256 kbps.
`--vbr` selects FDK's quality mode 5 with a 256 kbps peak limit. Its internal
nominal stereo rate is 192 kbps, but it does not target a fixed average bitrate:
the output depends on the audio. It cannot be combined with `--bitrate`.
The same 321-byte access-unit limit applies. CBR remains the default.
The worker validates the requested bitrate, generated core format
and each encoded packet before sending it.
This is lossy audio; it does not implement Apple's wireless lossless mode.

## Source and licensing

FDK-AAC 2.0.3 is pinned to
[`cac04476081a23c870fed05c36228cd02c5e7c3d`](https://github.com/mstorsjo/fdk-aac/tree/cac04476081a23c870fed05c36228cd02c5e7c3d).
The build verifies the source archive's SHA-256:

```text
1b572fcb5e2a81aa0447a92ca95bfe4979f3edfc09918e0627b726bcd0165ff1
```

The worker's original code uses the [MIT license](LICENSE). FDK's license is
retained in [NOTICE-FDK.txt](NOTICE-FDK.txt). Packages include both notices and
the complete pinned FDK source archive in `licenses/FDK-AAC-source.zip`.
[FFmpeg identifies FDK's license as incompatible with GPL v2 and v3](https://ffmpeg.org/doxygen/trunk/md_LICENSE.html).
The GPL backend communicates through pipes and does not link FDK. Separate
processes do not by themselves establish GPL compatibility for a combined product;
see the [FSF's aggregation guidance](https://www.gnu.org/licenses/gpl-faq.en.html#MereAggregation).
FDK grants no patent license.

Codec settings follow the no-SBR profile in
[USBPods' Pro 3 playback implementation](https://github.com/han-um/USBPods-Pico2W-2-way-audio-implement/blob/d18d083bc52de3143d8d922c4abaae865cc0dc28/src/btstack/btstack_avdtp_source.c#L1678-L1724),
with the selected bitrate capped at 256 kbps.
Successful local encoding does not establish AirPods playback or coexistence
with Windows audio.

The VBR option uses the rate-control API in the pinned FDK source:
`libAACenc/include/aacenc_lib.h` documents `AACENC_BITRATEMODE` and
`AACENC_PEAK_BITRATE`; `libAACenc/src/aacenc.cpp` defines mode 5's nominal rate.
The round-trip tests cover silence, stereo tones, noise and transients, plus the
per-frame size limit. This profile has not been validated on AirPods hardware.
