# FFmpeg source downloads

`FFmpeg-source.zip` contains the FFmpeg source used by the bundled 7.1 DLLs:
[`1fdbca85aaea513c9cc6c14d347f76543346d3da`](https://github.com/FFmpeg/FFmpeg/tree/1fdbca85aaea513c9cc6c14d347f76543346d3da).
`FFmpeg-build-recipes.zip` contains BtbN's build scripts, patches and dependency
source references at
[`a99e8230eae00d1cee38f23076a7a1f55cd984e2`](https://github.com/BtbN/FFmpeg-Builds/tree/a99e8230eae00d1cee38f23076a7a1f55cd984e2),
the recipe revision for
[`autobuild-2026-07-31-14-10`](https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-07-31-14-10).
`SHA256SUMS.txt` includes both archives and this note.

The recipe uses the `win64 lgpl-shared 7.1` target with Bash and Docker. Its
`build.sh` clones the moving `release/7.1` branch. To use the bundled version,
add this command after `cd ffmpeg` and before `./configure` in that script:

```sh
git checkout --detach 1fdbca85aaea513c9cc6c14d347f76543346d3da
```

Third-party dependency source archives are absent from these downloads. The
recipe's `scripts.d` directory records their source locations and versions;
building fetches those dependencies separately. These archives do not provide
a self-contained build environment or establish complete license compliance.
Keep the LGPL notice distributed with the FFmpeg DLLs and the notices in both
source archives.
