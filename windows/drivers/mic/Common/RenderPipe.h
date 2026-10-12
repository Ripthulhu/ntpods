#pragma once
#include <ntddk.h>
#include <wdf.h>
#include <acx.h>

#ifndef NTPODS_EXPERIMENTAL_PLAYBACK
#define NTPODS_EXPERIMENTAL_PLAYBACK 0
#endif

#define IOCTL_NTPODS_PLAYBACK_READ_PCM CTL_CODE(FILE_DEVICE_UNKNOWN, 0x900, METHOD_BUFFERED, FILE_READ_DATA)
#define IOCTL_NTPODS_PLAYBACK_STATUS CTL_CODE(FILE_DEVICE_UNKNOWN, 0x901, METHOD_BUFFERED, FILE_READ_DATA)
#define NTPODS_PLAYBACK_STATUS_VERSION 2u

// READ_PCM completes immediately with 0..1920 bytes of 48 kHz PCM16 stereo.
// STATUS returns these eight ULONGs. Both requests require empty input.
struct NTPODS_PLAYBACK_STATUS {
    ULONG Version, SampleRate, Channels, BitsPerSample;
    ULONG RunningStreams, QueuedFrames, DroppedFrames, Generation;
};

#if NTPODS_EXPERIMENTAL_PLAYBACK
VOID RenderPipeInit();
NTSTATUS RenderPipeCreateControlDevice(WDFDEVICE Parent);
VOID RenderPipeDeleteControlDevice(WDFDEVICE Parent);
VOID RenderPipeSetOnline(BOOLEAN Online);
VOID RenderPipeStarted();
VOID RenderPipeStopped();
VOID RenderPipeWrite(_In_reads_bytes_(Length) const void* Data, ULONG Length);
__declspec(code_seg("PAGE")) NTSTATUS CodecR_SetPacketConstraints(ACXCIRCUIT Circuit);
#endif
