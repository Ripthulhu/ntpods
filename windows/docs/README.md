# NTPods — Windows docs

Windows driver, daemon and protocol notes.

- **[`daemon-ipc/PLAN.md`](daemon-ipc/PLAN.md)** — the daemon + named-pipe IPC
  migration history and named-pipe design.
- **[`hires-mic/PLAN.md`](hires-mic/PLAN.md)** — the hi-res AirPods microphone:
  implementation history, AAC-ELD decoding and virtual audio driver checks.
- **[`heart-rate.md`](heart-rate.md)** — heart-rate monitoring: why it needed the
  AAP channel's MTU raised to 1691, what the daemon sends and decodes, and what was
  ruled out during testing.
- **[`aap-packet-discovery.md`](aap-packet-discovery.md)** — how to capture
  AAP traffic on macOS/iOS to confirm or discover packets.

Setup and usage live in the **[Windows README](../README.md)**; the drivers have
their own notes in [`../drivers/aap`](../drivers/aap) and
[`../drivers/mic`](../drivers/mic) (ACX, Windows 11 22H2+). The older
[`../drivers/mic-portcls`](../drivers/mic-portcls) source is retained for reference
and is not supported by this build.

## Other platforms

Linux and Android development is in the upstream
[LibrePods repository](https://github.com/librepods-org/librepods).

## Protocol (OS-agnostic)
AAP/AACP notes apply to every platform:
**[`AAP Definitions.md`](../../AAP%20Definitions.md)**,
**[`docs/control_commands.md`](../../docs/control_commands.md)**,
**[`docs/opcodes.md`](../../docs/opcodes.md)**,
**[`docs/device-info.md`](../../docs/device-info.md)**.
