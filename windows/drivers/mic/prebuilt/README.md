# Legacy microphone package

These upstream `AudioCodec` binaries predate the current access policy and PCM
bridge contract. They are retained for reference and are not packaged by the
build. Do not install them or combine them with the current daemon.

Use the MSI or ZIP from a complete [Windows build](../../../../docs/BUILD-WINDOWS.md).
It compiles the matching driver, regenerates its catalog and configures the
intended user's access before creating `ROOT\NTPodsMicACX`. The old single-driver
installer refuses to run because it bypasses that setup.
