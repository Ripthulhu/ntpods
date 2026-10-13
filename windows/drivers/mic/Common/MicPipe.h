/*++

Module Name:

    MicPipe.h

Abstract:

    The daemon writes decoded PCM to \\.\NTPodsMic. The ACX capture engine
    reads the shared ring once per notification tick.

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
// The first ULONG counts packets filled by the capture engine, including silence.
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

VOID MicPipeControlSddl(WDFDRIVER Driver, PWCHAR Buffer, size_t Characters);

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
// that exposes the PCM IOCTLs. Returns success if it already exists.
// Device add tolerates failure; the capture endpoint then produces silence.
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
// CCaptureStreamEngine::AllocateRtPackets supplies the capture packet size.
// Backlog trimming retains at least two packets, within MIC_RT_MAX_PACKET_BYTES.
//
VOID
MicPipeSetPacketSize(
    _In_ ULONG PacketSize
);

EXTERN_C_END
