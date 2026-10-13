/*
    Legacy PortCls/WaveRT microphone, excluded from the current installer.
    One mono 16-bit 48 kHz endpoint, fed through \\.\NTPodsMic with the
    WRITE_PCM and four-byte STATUS IOCTLs shared with the ACX driver.

    Files:
      driver.cpp  DriverEntry, IRP routing, adapter start, control device, ring
      wave.cpp    WaveRT miniport + capture stream (fills the cyclic buffer
                  from the ring as the audio engine polls the position)
      topo.cpp    topology miniport (one microphone pin bridged to the wave filter)
*/
#pragma once

#include <portcls.h>
#include <stdunk.h>
#include <ksmedia.h>

#define NTPODS_POOLTAG 'cMTN'

// Pins. Wave filter: 0 = bridge in (from topology), 1 = capture out (to the app).
// Topology filter: 0 = microphone in, 1 = bridge out (to wave).
#define WAVE_PIN_BRIDGE   0
#define WAVE_PIN_CAPTURE  1
#define TOPO_PIN_MIC      0
#define TOPO_PIN_BRIDGE   1

// The daemon supplies 48 kHz PCM16 mono after decoding and resampling.
#define MIC_RATE       48000
#define MIC_CHANNELS   1
#define MIC_BITS       16
#define MIC_BLOCK      (MIC_CHANNELS * MIC_BITS / 8)

// Shared with ../mic/Common/MicPipe.h; this legacy driver has no FLUSH IOCTL.
#define IOCTL_NTPODS_MIC_WRITE_PCM \
    CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_WRITE_DATA)
#define IOCTL_NTPODS_MIC_STATUS \
    CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_READ_DATA)

// Ring (driver.cpp).
VOID MicRingInit();
VOID MicRingWrite(_In_reads_bytes_(Len) const VOID* Data, _In_ ULONG Len);
VOID MicRingRead(_Out_writes_bytes_(Len) PVOID Out, _In_ ULONG Len);
VOID MicRingFlush();

// Miniport factories (wave.cpp, topo.cpp).
NTSTATUS CreateWaveMiniport(_Out_ PUNKNOWN* Unknown);
NTSTATUS CreateTopoMiniport(_Out_ PUNKNOWN* Unknown);

// Kernel new for the COM-style objects (stdunk.h brings delete).
PVOID operator new(size_t Size, POOL_FLAGS Flags, ULONG Tag);
