/*++

Module Name:

    MicPipe.h

Abstract:

    NTPods hi-res microphone bridge (Phase 2). A global ring buffer that
    user mode fills with decoded PCM over an IOCTL, and the ACX capture stream
    engine drains one packet per notification tick (see StreamEngine.cpp
    ProcessPacket). Exposed to user mode through a control device
    (\\.\NTPodsMic).

    PCM format: mono, 16-bit, 48000 Hz.

Environment:

    Kernel mode

--*/

#pragma once

#include <ntddk.h>
#include <wdf.h>

//
// IOCTL: user mode pushes raw PCM bytes into the mic ring buffer.
// Value (precomputed for the user-mode side): 0x0022A000.
//   CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_WRITE_DATA)
//
#define IOCTL_NTPODS_MIC_WRITE_PCM \
    CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_WRITE_DATA)

//
// IOCTL: user mode reads a capture-activity counter (ULONG). It advances every
// time an app pulls a capture packet, so the tray can tell when the mic is
// actually being recorded and auto-enable/disable the hi-res stream.
// If the output buffer holds 8 bytes, a second ULONG reports running capture
// streams; the original 4-byte result is retained for older callers.
// Value (precomputed): 0x00226004.
//   CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_READ_DATA)
//
#define IOCTL_NTPODS_MIC_STATUS \
    CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_READ_DATA)

// No input/output buffers. Drop queued speech without removing the endpoint.
// Value: 0x0022A008.
#define IOCTL_NTPODS_MIC_FLUSH \
    CTL_CODE(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_WRITE_DATA)

EXTERN_C_START

//
// Initialize the global ring buffer. Call once, early (device add), before any
// read/write. Idempotent.
//
VOID
MicPipeInit(
    VOID
);

//
// Create the control device (\Device\NTPodsMic + \DosDevices\NTPodsMic)
// that exposes IOCTL_NTPODS_MIC_WRITE_PCM. Driver-scoped and created once;
// safe to call again (returns STATUS_SUCCESS if already created). Best-effort:
// a failure here must not fail device add (the mic still enumerates, just with
// no user-mode feed yet).
//
NTSTATUS
MicPipeCreateControlDevice(
    _In_ WDFDEVICE Parent
);

// Called at PASSIVE_LEVEL from the owning PnP device's cleanup callback.
VOID MicPipeDeleteControlDevice(_In_ WDFDEVICE Parent);

VOID
MicPipeFlush(
    VOID
);

// Paired once per successful capture RUN/PAUSE transition.
VOID MicPipeCaptureStarted(VOID);
VOID MicPipeCaptureStopped(VOID);

//
// Fill Out with Len bytes from the ring (called from ProcessPacket, up to
// DISPATCH_LEVEL). Any underrun is zero-filled (silence).
//
VOID
MicPipeRead(
    _Out_writes_bytes_(Len) PVOID Out,
    _In_                    ULONG Len
);

//
// Publish how many bytes the capture side pulls per tick (called from
// CStreamEngine::AllocateRtPackets). The ring's latency trim keeps at least two
// of these buffered, so a client that captures in large chunks doesn't underrun
// on every read.
//
VOID
MicPipeSetPacketSize(
    _In_ ULONG PacketSize
);

EXTERN_C_END
