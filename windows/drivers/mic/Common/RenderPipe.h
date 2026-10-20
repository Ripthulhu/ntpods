#pragma once
#include <ntddk.h>
#include <wdf.h>
#include <acx.h>
#include "RenderDiagnostics.h"

#ifndef NTPODS_EXPERIMENTAL_PLAYBACK
#define NTPODS_EXPERIMENTAL_PLAYBACK 0
#endif

#define IOCTL_NTPODS_PLAYBACK_READ_PCM CTL_CODE(FILE_DEVICE_UNKNOWN, 0x900, METHOD_BUFFERED, FILE_READ_DATA)
#define IOCTL_NTPODS_PLAYBACK_STATUS CTL_CODE(FILE_DEVICE_UNKNOWN, 0x901, METHOD_BUFFERED, FILE_READ_DATA)
#define IOCTL_NTPODS_PLAYBACK_SET_PROVIDER CTL_CODE(FILE_DEVICE_UNKNOWN, 0x902, METHOD_BUFFERED, FILE_WRITE_DATA)
#define IOCTL_NTPODS_PLAYBACK_SET_CLOCK CTL_CODE(FILE_DEVICE_UNKNOWN, 0x903, METHOD_BUFFERED, FILE_WRITE_DATA)
#define IOCTL_NTPODS_PLAYBACK_QUERY_CLOCK CTL_CODE(FILE_DEVICE_UNKNOWN, 0x904, METHOD_BUFFERED, FILE_READ_DATA)
#define IOCTL_NTPODS_PLAYBACK_WAIT_CHANGE CTL_CODE(FILE_DEVICE_UNKNOWN, 0x905, METHOD_BUFFERED, FILE_READ_DATA)
#define IOCTL_NTPODS_PLAYBACK_DIAGNOSTICS CTL_CODE(FILE_DEVICE_UNKNOWN, 0x906, METHOD_BUFFERED, FILE_READ_DATA)
#define NTPODS_PLAYBACK_STATUS_VERSION 4u

// Version 4 READ_PCM returns 0..3840 bytes of 48 kHz stereo IEEE FLOAT32.
// READ_PCM and STATUS require empty input. SET_PROVIDER requires version 1.
struct NTPODS_PLAYBACK_STATUS {
    ULONG Version, SampleRate, Channels, BitsPerSample;
    ULONG RunningStreams, QueuedFrames, DroppedFrames, Generation;
    ULONGLONG Address;
    ULONG ProviderReady, Online;
};
struct NTPODS_PLAYBACK_PROVIDER { ULONG Version, Ready; };
// Sequence 0 queries immediately; otherwise wait for PCM or producer-state changes.
// One wait per provider handle. Returns a nonzero ULONG sequence; cancellation is safe.
struct NTPODS_PLAYBACK_WAIT_CHANGE { ULONG Version, Sequence; };
// Clock ABI version 1. IntervalTenths 9/11 needs DeltaMs 1..500; 10/0
// cancels without removing accrued virtual-time offset. Generation must match.
// Phase: 0 idle, 1 armed, 2 active, 3 completed, 4 expired, 5 cancelled.
// Requested interval/budget and final frame counters survive cancel/completion;
// a source-generation reset clears them. All times are QPC-derived 100 ns units.
struct NTPODS_PLAYBACK_CLOCK_SET { ULONG Version, Generation, IntervalTenths, DeltaMs; };
struct NTPODS_PLAYBACK_CLOCK_SNAPSHOT {
    ULONG Version, Generation, EffectiveIntervalTenths, Phase;
    ULONGLONG RealTimeHns, VirtualTimeHns;
    ULONG RequestedIntervalTenths, DeltaMs;
    ULONGLONG ActualFrames, NominalFrames, DeadlineRealTimeHns;
};

#if NTPODS_EXPERIMENTAL_PLAYBACK
NTSTATUS RenderPipeInit(WDFDEVICE Device);
NTSTATUS RenderPipeCreateControlDevice(WDFDEVICE Parent, ULONGLONG Address);
VOID RenderPipeDeleteControlDevice(WDFDEVICE Parent);
VOID RenderPipeSetOnline(WDFDEVICE Device, BOOLEAN Online);
VOID RenderPipeSetJack(WDFDEVICE Device, ACXJACK Jack);
BOOLEAN RenderPipeConnected(WDFDEVICE Device);
VOID RenderPipeStarted(WDFDEVICE Device);
VOID RenderPipeStopped(WDFDEVICE Device);
VOID RenderPipeWrite(WDFDEVICE Device, _In_reads_bytes_(Length) const void* Data,
    ULONG Length, ULONG SourceFrameBytes);
VOID RenderPipeRecordVolume(WDFDEVICE Device, const RenderVolumeState* State);
VOID RenderPipeRecordPacket(WDFDEVICE Device, ULONG VolumeFlags, BOOLEAN Missing,
    ULONG SubmittedBytes, BOOLEAN Nonzero);
ULONGLONG RenderPipeAudioTime(WDFDEVICE Device, ULONGLONG RealTimeHns);
LONGLONG RenderPipeTimerDelay(WDFDEVICE Device, ULONGLONG VirtualDueHns, ULONGLONG* GlitchHns);
__declspec(code_seg("PAGE")) NTSTATUS CodecR_SetPacketConstraints(ACXCIRCUIT Circuit);
VOID CodecR_ResetVolumeBridge(ACXCIRCUIT Circuit);
#endif
