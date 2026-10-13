# Legacy AAP package files

This directory is retained from upstream and is not used by the current build.
It does not contain a complete, current driver package. Do not install it or
combine it with the current daemon.

Use the MSI or ZIP from a complete [Windows build](../../../../docs/BUILD-WINDOWS.md).
That build compiles the driver from source; its installer sets the intended
user's access policy before loading it. The old single-driver installer refuses
to run because it bypasses that policy.
