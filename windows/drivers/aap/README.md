# NTPodsAAP — Windows AAP L2CAP driver

Open-source KMDF Bluetooth **profile driver** that lets Windows talk to AirPods
over Apple's Accessory Protocol (AAP). AAP runs on a classic-Bluetooth **L2CAP
channel at PSM `0x1001`**, which Windows user-mode Winsock cannot open (only
RFCOMM is exposed; raw L2CAP `connect()` fails with `WSAENETDOWN`). This driver
opens that channel in kernel mode and bridges it to user space via
`DeviceIoControl`, so NTPods can read battery levels and change listening modes.

## Development build

Build from source; the checked-in prebuilt package does not contain these changes.
The elevated installer must set the `AllowedUserSid` REG_SZ value under
`HKLM\SYSTEM\CurrentControlSet\Services\NTPodsAAP\Parameters` before binding the
driver. Its protected registry key may be written only by administrators/SYSTEM.
The device permits SYSTEM and that one local/domain account (`S-1-5-21-...`). A
missing, malformed, or unsupported account SID leaves the interface SYSTEM-only.

The IOCTL numbers and packed payloads remain compatible with the daemon. CONNECT
accepts only PSM `0x1001` and the peer reported by the owning BTHENUM service PDO;
SEND/RECEIVE payloads are limited to 8192 bytes. A receive waits at most 100 ms,
including when a caller requests zero or a longer timeout; callers should poll.
Timeouts return an empty receive. Cancellation is checked every 50 ms while a BRB
is pending. Connect has a 10-second deadline, other BRBs a 2-second outer deadline.
On cancellation/deadline, the child request is canceled and must complete before
its buffers are released. A malfunctioning lower driver that never completes a
canceled IRP cannot safely be abandoned by this driver.

Handle cleanup and PnP suspend close both AAP and ATT, using the address saved for
each channel. The unused ATT server has been removed; legacy server-status fields
remain zero/unsupported. The driver only opens an outbound ATT client on demand.

A parallel dispatch queue forwards requests into four sequential queues: control,
AAP receive, ATT receive, and cached status. A waiting ATT read/open or AAP write
does not block kernel AAP receives or `GET_STATUS`. Status takes only the brief
state spin lock and never borrows a Bluetooth interface or channel. Each BRB checks cancellation of
its own parent request. Close waits for the corresponding receive lock before
releasing a channel; control operations remain serialized by `OperationLock`.
Lock order is control, then one receive lock, then the brief state spin lock.
Receive handlers never acquire the control lock, and callbacks only take the
state spin lock. A remote AAP disconnect also marks ATT unavailable until reopen.

The Rust transport opens the device with `FILE_FLAG_OVERLAPPED`, using a separate
manual-reset event and `OVERLAPPED` per call. Calls request cancellation after
20 seconds, including queueing time, and wait for completion before releasing any
buffers. This requires a completing lower stack, just like the kernel BRB wait.
The old synchronous handle would serialize calls before the separate driver
queues could help. The IOCTL ABI and account/device restrictions are unchanged.

Portable boundary check (no driver installation):

```
cc -std=c11 -Wall -Wextra -Werror tests/policy_test.c -o policy_test
./policy_test
```

The host-side concurrency harness compiles the production IOCTL callbacks against
small framework substitutes. It checks independent AAP/ATT reads, writes while
reads are pending, per-request cancellation, and receive input validation:

```
cl /nologo /W4 /WX /EHsc /std:c++17 tests/receive_concurrency_test.cpp
receive_concurrency_test.exe
```

`cargo test --manifest-path windows/daemon/Cargo.toml driver::tests` exercises
Windows overlapped completion on local test pipes. One pending read times out and
is drained while another on the same handle remains usable; a closed peer returns
an error without waiting for the deadline. Neither test needs an installed driver.

Before loading a release, run Driver Verifier and the WDK checks on a dedicated
test system. Validate malformed/oversized IOCTLs, cancellation during a receive
and connect, access rejection from another user, repeated open/close and daemon
crash, ATT close/reopen, sleep/resume, radio removal, and driver uninstall. Confirm
ANC/STOP sends remain responsive during idle receive polling, and that the
Windows audio profiles keep working. These hardware checks are not replaced by
the portable policy test.

## Transport research, 2026-10-11

- Microsoft's [L2CAP transfer contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/bthddi/ns-bthddi-_brb_l2ca_acl_transfer)
  allows reads and writes simultaneously. Its [channel-open documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/bthddi/ns-bthddi-_brb_l2ca_open_channel)
  states that incoming packets are discarded when the configured MTU queue fills.
  Serializing every operation behind a long ATT open could therefore lose AAP
  microphone frames. The new receive queues remove that local source of delay;
  they cannot repair radio loss or time spent decoding in user mode.
- [WdfRequestForwardToIoQueue](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdfrequest/nf-wdfrequest-wdfrequestforwardtoioqueue)
  returns queued request ownership to the framework. The forwarding queue must
  remain able to accept the next receive while a control operation is outstanding.
- [DeviceIoControl](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-deviceiocontrol),
  [GetOverlappedResultEx](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-getoverlappedresultex)
  and [CancelIoEx](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelioex)
  define the per-operation event and completion lifetime used by the daemon.
- [Microsoft bthecho](https://github.com/microsoft/Windows-driver-samples/blob/main/bluetooth/bthecho/bthcli/sys/queue.c)
  reports received bytes from `BufferSize`. [BthPS3 at 2a0de63](https://github.com/nefarius/BthPS3/blob/2a0de63ce48092a20cfe12d0600e3619688de743/BthPS3/L2CAP.Transfer.c)
  uses asynchronous BRBs and explicit rundown, but subtracts `RemainingBufferSize`.
  The deployed byte-count interpretation is retained until a hardware trace
  resolves that discrepancy. No code was copied from either reference.

Pending hardware checks: AAP packet continuity during ATT settings/probes, send
latency under microphone capture, cancel/disconnect during both receive types,
daemon crash, radio removal and sleep/resume with Driver Verifier. The current
MTU and ten-MTU input queue remain unchanged; larger buffers would conceal stalls
and do not replace those checks.

## How it binds

AirPods advertise an SDP service with UUID `{74ec2172-0bad-4d01-8f77-997b2be0722a}`
(taken from LibrePods on Linux). Windows enumerates a devnode
`BTHENUM\{74ec2172-...}_VID&0001004c_PID&2027` for it. Our INF matches that
hardware ID, so Windows loads this driver as the **function driver for the AAP
service PDO**, with the Bluetooth stack as its parent I/O target. It calls
`BthAllocateBrb` and submits `BRB_L2CA_OPEN_CHANNEL` to open PSM `0x1001` outbound.
No filter driver is needed (unlike PS3/BthPS3), because AirPods advertise the
service. Architecture reference: MS `bthecho` sample + `nefarius/BthPS3`.

The INF leaves `ProductType` unrestricted so both ordinary Windows 11 and
Enterprise multi-session can select the model. Multi-session reports product
type 3; the old workstation-only `NTamd64.10.0.1..16299` excluded it. The x64
architecture and existing minimum build 16299 are preserved. See Microsoft's
[TargetOSVersion documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/inf-manufacturer-section).

The native regression test asks SetupAPI to select the real INF for workstation
and multi-session product types, and rejects older builds and other architectures.
From an x64 Developer prompt (read-only; no device installation):

```
cl /nologo /W4 /WX tests/inf_target_test.c /link setupapi.lib
inf_target_test.exe NTPodsAAP.inf
```

## Files

| File | Role |
|------|------|
| `NTPodsAAP.h` | IOCTL contract, device context, prototypes |
| `Driver.c` | `DriverEntry`, device creation, IOCTL queue |
| `Device.c` | PnP: query `BTH_PROFILE_DRIVER_INTERFACE`, get I/O target |
| `L2cap.c` | connect / disconnect / send / receive via BRBs |
| `Ioctl.c` | user-mode bridge (DeviceIoControl → L2CAP) |
| `NTPodsAAP.inf` | binds to the AAP service, installs KMDF service |
| `NTPodsAAP.vcxproj` | KMDF x64 project |

## IOCTL contract (for the user-mode transport)

Device interface GUID `{C0FFEE00-1337-4A5B-9E6F-A1B2C3D4E5F6}`, `FILE_DEVICE 0x8000`:
`CONNECT 0x800 {u64 addr; u16 psm}`, `DISCONNECT 0x801`, `SEND 0x802` (raw bytes),
`RECEIVE 0x803` (raw bytes out), `GET_STATUS 0x804`.

## Build

Needs VS2022/2026 with the C++ workload + Windows SDK/WDK (matching build numbers,
e.g. 28000). From a Developer prompt:

```
msbuild NTPodsAAP.vcxproj /p:Configuration=Release /p:Platform=x64
```

Then generate the catalog from a folder holding `NTPodsAAP.sys` + `.inf`:

```
inf2cat /driver:<pkg-dir> /os:10_X64
```

## Install and validation

Use `NTPods.msi` or `install.ps1` from a complete generated distribution. The
central installer sets the intended user's protected registry value before the
driver is bound, verifies the package, and installs both drivers together. The
old single-driver `install.ps1` deliberately fails without changing the machine.
Do not install the upstream `prebuilt/` binaries: they lack this hardening.

Initial AirPods hardware checks are recorded in
[WINDOWS-VALIDATION.md](../../../docs/WINDOWS-VALIDATION.md), including the remaining
capture and recovery failures. A test package needs a prepared test system; a
normal release needs Microsoft driver signing and the hardware checks listed above.

The upstream transport uses L2CAP Basic mode. ATT (PSM `0x001F`) opens lazily on the
first ATT send and closes with AAP on disconnect or handle cleanup.

## Experimental playback transport

`/p:NTPodsExperimentalPlayback=true` adds separate AVDTP signaling and media
channels. Normal builds exclude this code. Existing AAP/ATT IOCTLs are unchanged.
Both channels use PSM `0x0019`, the paired device belonging to this PDO, and an
authenticated, encrypted link. There is no caller-supplied PSM.

The backend must release the existing playback transport before opening these
channels. This driver cannot control another driver's AVDTP session. It supplies
L2CAP transport; the backend owns AVDTP negotiation, media framing and recovery.

The version-1 ABI uses packed little-endian fields and requires read/write access
to the existing device handle:

| IOCTL | Input | Output |
| --- | --- | --- |
| `0x8000E040` OPEN | `u32 version, u32 channel, u64 address` | `u32 version, u32 channel, u32 tx_mtu, u32 rx_mtu` |
| `0x8000E044` CLOSE | none | none |
| `0x8000E048` SEND | `u32 version, u32 channel`, then packet | none |
| `0x8000E04C` RECEIVE | `u32 version, u32 channel, u32 timeout_ms` | packet bytes |
| `0x8000E050` STATUS | none | `u32 version, u32 signaling_open, u32 media_open, u32 signaling_tx_mtu, u32 media_tx_mtu, u64 address` |

Channel 0 is signaling; channel 1 is media and requires signaling to be open.
Packet buffers are capped at 8192 bytes. Sends must fit the negotiated outbound
MTU; receive buffers must hold the inbound MTU. Receives wait at most 100 ms and
return zero bytes on timeout. The existing BRB cancellation and completion rules
also apply here. Status does not wait for pending Bluetooth I/O.

Media requests a 200 ms outbound L2CAP flush timeout; signaling requires infinite
retransmission. An unexpected negotiated timeout rejects the channel. Windows
owns the HCI packet flags: the BRB transfer API has no per-packet auto-flush flag.
Hardware captures must verify expiry and signaling reliability under congestion.

CLOSE closes media before signaling, leaving AAP and ATT alone. File cleanup,
suspend and removal close all channels. An unexpected signaling disconnect
disables media immediately and queues its closure at passive level. Separate
send/receive locks prevent local close from releasing an in-flight transfer's
channel. Failed closes retain the handle for cleanup retry unless the peer or
radio has already removed it.

`tests/playback_transport_test.cpp` compiles the production transport with WDF
and completing Bluetooth substitutes. It checks access boundaries, MTUs,
concurrent I/O, cancellation, failed opens/closes and remote disconnect cleanup.
Build it with `cl /W4 /WX /EHsc /std:c++17` like the existing concurrency test.
Hardware transport, codec playback and Driver Verifier remain untested.
