# Windows AVRCP volume bridge

NTPods uses Windows' existing AVRCP connection to synchronize the AirPods'
absolute volume with the native audio endpoint. The render driver exposes the
local volume interface used by `BthAvrcp.dll`; Windows continues to handle
Bluetooth volume commands, notifications and media controls.

This is an unpublished driver interface, identified by static inspection of
these Windows binaries and their matching public symbols:

| Binary | File version | Matching PDB GUID, age |
| --- | --- | --- |
| `BthAvrcp.dll` | `10.0.26100.9278` | `3C04B769-5D95-56B7-516C-032092E7DB96`, 1 |
| `BthA2dp.sys` | `10.0.26100.7920` | `39745226-31E6-CD26-49EA-7AC2AAA847D1`, 1 |

NTPods 0.2.29 passed Pro 3 / 9A348 checks on Intel AX200 for stem-to-Windows
synchronization, audible Windows volume changes, mute/unmute and volume
preservation across AAC-to-AAC-ELD switching. Idle, reconnect, other models and
other Windows builds need testing; see [validation](WINDOWS-VALIDATION.md).

The implementation is in [RenderCircuit.cpp](../windows/drivers/mic/Common/RenderCircuit.cpp)
and [RenderVolume.h](../windows/drivers/mic/Common/RenderVolume.h). The microphone
circuit does not use this bridge.

## Discovery and requests

`VolumeController::CreateA2dpDeviceWatcher` finds the present AudioSink service
devnode by `DEVPKEY_Bluetooth_ServiceGUID` (UUID `0x110B`) and
`DEVPKEY_Bluetooth_DeviceAddress`. `CreateLocalVolumeInterfaceWatcher` then finds
an enabled interface on that same devnode in category
`e630310c-1733-42bb-9847-7e45d1a27490`. The interface must belong to that AudioSink
devnode.

`AcxCircuitInitAssignCategories` replaces the default list. Keep ACX's internal
circuit category, `2c6bb644-e1ae-47f8-9a2b-1d1fa750f2fa`, first, followed by the
audio categories and the volume category. The inspected runtime rejects a
different first category with `STATUS_INVALID_DEVICE_STATE` during startup.

Property and event requests use set
`401e566b-b47e-4e34-b0fc-94826e9e2a1a`, ID `1`:

| Request | Header and data | Direction and meaning |
| --- | --- | --- |
| `IOCTL_KS_PROPERTY` (`0x2F0003`), GET (`1`) | 24-byte `KSPROPERTY`; four-byte signed `LONG` | Windows reads the local cache in 16.16 dB |
| `IOCTL_KS_PROPERTY`, SET (`2`) | Same layout; no channel field | Windows publishes its cached AVRCP volume to the driver |
| `IOCTL_KS_ENABLE_EVENT` (`0x2F0007`) | 24-byte `KSEVENT`, `KSEVENT_TYPE_ENABLE` (`1`); `KSEVENTDATA` with `KSEVENTF_EVENT_HANDLE` | Driver signals a local cache change for Windows to read |

Native x64 `KSEVENTDATA` is 32 bytes. ACX owns event registration and removal;
the driver must not retain a raw user event handle. The inbox A2DP driver permits
one subscriber and rejects a duplicate with `STATUS_ALREADY_REGISTERED`.

Keep `ACX_EVENT_CONFIG.Type` zero when creating the event. ACX rejects a
nonzero value with `STATUS_INVALID_PARAMETER`; `KSEVENT_TYPE_ENABLE` belongs
in the client's subscription request, not the ACX event configuration.

Host volume/mute writes update the shared cache immediately and signal the
private event. `OnLocalVolumeChangedInternal` reads it, converts to AVRCP's
0–127 scale and sends `SetAbsoluteVolume` only when it differs from Windows'
cached remote value. Remote cache writes notify the ACX volume/mute controls
so Core Audio can update the slider. The inbox driver also signals the private
event for these writes; Windows suppresses the echo by comparing its cache.

## Gain and mute

The advertised range is −63.5 to 0 dB, with a 0.5 dB step:
minimum `−4161536` (`0xFFC08000`), maximum `0`, step `32768` (`0x8000`) in 16.16
units. Inbox A2DP clamps writes to this range without rounding them to the
advertised step. Do not round remote values to 0.5 dB before caching them.

Windows uses a nonlinear taper. For AVRCP value `v` in `0..127`, define
`f = 10^(−63.5/35)`:

```text
dB(v) = 35 × log10(f + (1 − f) × v / 127)
v(dB) = round(127 × (10^(dB/35) − f) / (1 − f))
```

Clamp to the stated ranges; the private property carries dB multiplied by
65536. A percentage, AVRCP step and dB value are not interchangeable. The
daemon's [volume ownership logic](../windows/daemon/src/volume.rs) compares
AVRCP steps when recognizing a quantized echo of its own temporary change.

There is one shared gain and mute state. Inbox A2DP accepts channels `−1`, `0`
and `1` but advertises two uniform channels: basic-support member flags
include `KSPROPERTY_MEMBER_FLAG_BASICSUPPORT_MULTICHANNEL` and
`KSPROPERTY_MEMBER_FLAG_BASICSUPPORT_UNIFORM`. Do not advertise independent
left/right gain while implementing one cache.

Preserve these inbox mute rules:

- Standard volume GET returns the remembered level, including while muted.
  Private GET returns the minimum while muted.
- A changed level above the minimum clears mute. A host write of the minimum
  remembers that level and sets mute.
- A remote minimum write while already muted preserves the remembered level,
  allowing a later unmute to restore it. Otherwise it remembers the minimum
  and sets mute.
- A write equal to the remembered level is a no-op before mute handling.
  An explicit mute change preserves the remembered level.

The earbuds apply the gain. Do not apply the same attenuation again to PCM.
Local silence for mute or an unavailable bridge does not implement a second
volume curve.

## Startup and lifetime

`StartCategory2Controllers` starts the volume-notification controller and then
creates `VolumeController` without waiting for a remote interim response. Its
state machine proceeds through local AudioSink arrival, interface arrival and
event subscription. `EnableResponseProcessingEntry` then unconditionally writes
the cached AVRCP level through the private SET property.

The cache initially contains AVRCP value `64`; an earlier interim can replace
it. The first private SET establishes bridge initialization. This property
cannot distinguish an initial cached write from a fresh earbud volume report.

Equal later interims can be skipped. An accepted `SetAbsoluteVolume` response
returns the state machine to awaiting changes without another private SET.
Do not clear bridge readiness on host writes or ordinary render D0 idle and
expect either response to restore it.

A normal AVRCP control-channel disconnect releases the controller lifetime;
`VolumeController` stops its state machine, destroys the event watcher and
closes the KS interface. Clear subscription readiness on removal or teardown,
and preserve the remembered gain/mute across ordinary idle and codec changes.
Use subscription lifetime, not an audio power transition, as the reset boundary.

[Volume tests](../windows/drivers/mic/tests/render_volume_test.cpp) cover cache
and mute behavior. Windows discovery, acoustic gain, startup ordering and
reconnect behavior require hardware checks.
