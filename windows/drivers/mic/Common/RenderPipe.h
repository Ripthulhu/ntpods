#pragma once
#include <ntddk.h>
#include <wdf.h>
#include <acx.h>

#ifndef NTPODS_EXPERIMENTAL_PLAYBACK
#define NTPODS_EXPERIMENTAL_PLAYBACK 0
#endif

#define IOCTL_NTPODS_PLAYBACK_READ_PCM CTL_CODE(FILE_DEVICE_UNKNOWN, 0x900, METHOD_BUFFERED, FILE_READ_DATA)
#define IOCTL_NTPODS_PLAYBACK_STATUS CTL_CODE(FILE_DEVICE_UNKNOWN, 0x901, METHOD_BUFFERED, FILE_READ_DATA)
#define IOCTL_NTPODS_PLAYBACK_SET_PROVIDER CTL_CODE(FILE_DEVICE_UNKNOWN, 0x902, METHOD_BUFFERED, FILE_WRITE_DATA)
#define NTPODS_PLAYBACK_STATUS_VERSION 3u

// READ_PCM completes immediately with 0..1920 bytes of 48 kHz PCM16 stereo.
// READ_PCM and STATUS require empty input. SET_PROVIDER requires version 1.
struct NTPODS_PLAYBACK_STATUS {
    ULONG Version, SampleRate, Channels, BitsPerSample;
    ULONG RunningStreams, QueuedFrames, DroppedFrames, Generation;
    ULONGLONG Address;
    ULONG ProviderReady, Online;
};
struct NTPODS_PLAYBACK_PROVIDER { ULONG Version, Ready; };

#if NTPODS_EXPERIMENTAL_PLAYBACK
NTSTATUS RenderPipeInit(WDFDEVICE Device);
NTSTATUS RenderPipeCreateControlDevice(WDFDEVICE Parent, ULONGLONG Address);
VOID RenderPipeDeleteControlDevice(WDFDEVICE Parent);
VOID RenderPipeSetOnline(WDFDEVICE Device, BOOLEAN Online);
VOID RenderPipeSetJack(WDFDEVICE Device, ACXJACK Jack);
BOOLEAN RenderPipeConnected(WDFDEVICE Device);
VOID RenderPipeStarted(WDFDEVICE Device);
VOID RenderPipeStopped(WDFDEVICE Device);
VOID RenderPipeWrite(WDFDEVICE Device, _In_reads_bytes_(Length) const void* Data, ULONG Length);
__declspec(code_seg("PAGE")) NTSTATUS CodecR_SetPacketConstraints(ACXCIRCUIT Circuit);
#endif
