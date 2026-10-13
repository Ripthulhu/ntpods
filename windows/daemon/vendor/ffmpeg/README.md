# FFmpeg build dependencies

NTPods uses FFmpeg 7.1's `avcodec`, `avutil` and `swresample` libraries for AAC-ELD
microphone decoding and AAC playback encoding. Headers, import libraries and DLLs
are downloaded during the build and are not committed.

From the repository root, run:

```sh
bash windows/daemon/fetch-ffmpeg.sh
```

The script verifies a pinned [BtbN shared build](https://github.com/BtbN/FFmpeg-Builds)
and writes headers to `include/`, MSVC and MinGW import libraries to `lib/`, and
runtime DLLs to `bin/`. Packages ship `avcodec-61.dll`, `avutil-59.dll` and
`swresample-5.dll` beside the app.

This bundle is LGPL v3 or later (`--enable-version3`). Keep `LICENSE-ffmpeg.txt`
and the bundled notices with it. The matching source and build recipes are
documented in [FFMPEG-SOURCES.md](../../../../docs/FFMPEG-SOURCES.md).

When updating, change `URL` and `URL_SHA256` in `fetch-ffmpeg.sh`, the pinned
archives in `windows/fetch-release-sources.ps1`, and the source notes together.
Keep the DLL names in the build and packaging scripts matched to the new version.
