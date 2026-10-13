# Windows documentation

Start with the [Windows guide](../README.md) for setup and usage, or the
[build guide](../../docs/BUILD-WINDOWS.md) for development.

| Topic | Reference |
| --- | --- |
| Daemon ownership and named-pipe IPC | [Daemon and IPC](daemon-ipc/PLAN.md) |
| Microphone decoding and recovery | [Microphone architecture](hires-mic/PLAN.md) |
| Heart-rate channel, stream and report format | [Heart rate](heart-rate.md) |
| Capturing and inspecting Bluetooth traffic | [Packet discovery](aap-packet-discovery.md) |
| Current protocol implementation | [AirPods protocol](../../docs/AIRPODS-PROTOCOL.md) |
| AAP commands and packet layouts | [AAP definitions](../../docs/AAP%20Definitions.md) |

Driver contracts are documented in [AAP](../drivers/aap/README.md) and
[microphone](../drivers/mic/README.md); the [playback guide](../playback/README.md)
explains the AudioSink driver and codecs. The older `drivers/mic-portcls` source
is retained for reference and is not supported by this build.

Linux and Android development continues in
[LibrePods](https://github.com/librepods-org/librepods).
