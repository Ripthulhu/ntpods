#pragma once

#include <stddef.h>
#if defined(_NTDDK_)
typedef ULONG LP_PLAYBACK_U32;
typedef ULONGLONG LP_PLAYBACK_U64;
#else
#include <stdint.h>
typedef uint32_t LP_PLAYBACK_U32;
typedef uint64_t LP_PLAYBACK_U64;
#endif

#define LP_PLAYBACK_VERSION 1u
#define LP_PLAYBACK_OPEN_VERSION 2u
#define LP_PLAYBACK_MEDIA_FLUSH_MS 200u
#define LP_PLAYBACK_SIGNALING 0u
#define LP_PLAYBACK_MEDIA 1u
#define LP_PLAYBACK_CHANNELS 2u
#define LP_PLAYBACK_MAX_PACKET 8192u

#define LP_PLAYBACK_MPM_VERSION 1u
#define LP_PLAYBACK_MPM_QUERY 0u
#define LP_PLAYBACK_MPM_ENABLE 1u
#define LP_PLAYBACK_MPM_REPORT 2u
#define LP_PLAYBACK_MPM_DISABLE 3u
#define LP_PLAYBACK_MPM_ACTUAL_SUSPENDED 1u
#define LP_PLAYBACK_MPM_ACTUAL_PRODUCER_PLAYING 2u
#define LP_PLAYBACK_MPM_STATE_ENABLED 1u
#define LP_PLAYBACK_MPM_STATE_SUSPEND_REQUESTED 2u
#define LP_PLAYBACK_MPM_STATE_REQUEST_PENDING 4u

#pragma pack(push, 1)
typedef struct _LP_PLAYBACK_OPEN_INPUT {
    LP_PLAYBACK_U32 Version;
    LP_PLAYBACK_U32 Channel;
    LP_PLAYBACK_U64 Address;
} LP_PLAYBACK_OPEN_INPUT;

typedef struct _LP_PLAYBACK_OPEN_INPUT_V2 {
    LP_PLAYBACK_OPEN_INPUT Base;
    LP_PLAYBACK_U32 FlushTimeoutMs;
} LP_PLAYBACK_OPEN_INPUT_V2;

typedef struct _LP_PLAYBACK_CLOSE_INPUT {
    LP_PLAYBACK_U32 Version;
    LP_PLAYBACK_U32 Channel;
} LP_PLAYBACK_CLOSE_INPUT;

typedef struct _LP_PLAYBACK_OPEN_OUTPUT {
    LP_PLAYBACK_U32 Version;
    LP_PLAYBACK_U32 Channel;
    LP_PLAYBACK_U32 TransmitMtu;
    LP_PLAYBACK_U32 ReceiveMtu;
} LP_PLAYBACK_OPEN_OUTPUT;

typedef struct _LP_PLAYBACK_SEND_INPUT {
    LP_PLAYBACK_U32 Version;
    LP_PLAYBACK_U32 Channel;
} LP_PLAYBACK_SEND_INPUT;

typedef struct _LP_PLAYBACK_RECEIVE_INPUT {
    LP_PLAYBACK_U32 Version;
    LP_PLAYBACK_U32 Channel;
    LP_PLAYBACK_U32 TimeoutMs;
} LP_PLAYBACK_RECEIVE_INPUT;

typedef struct _LP_PLAYBACK_STATUS_OUTPUT {
    LP_PLAYBACK_U32 Version;
    LP_PLAYBACK_U32 SignalingOpen;
    LP_PLAYBACK_U32 MediaOpen;
    LP_PLAYBACK_U32 SignalingTransmitMtu;
    LP_PLAYBACK_U32 MediaTransmitMtu;
    LP_PLAYBACK_U64 Address;
} LP_PLAYBACK_STATUS_OUTPUT;

/* Suspension is the applied coordinator policy, independent of producer RUN. */
typedef struct _LP_PLAYBACK_MPM_INPUT {
    LP_PLAYBACK_U32 Version;
    LP_PLAYBACK_U32 Operation;
    LP_PLAYBACK_U32 SignalingGeneration;
    LP_PLAYBACK_U32 AckSequence;
    LP_PLAYBACK_U32 ActualFlags;
} LP_PLAYBACK_MPM_INPUT;

typedef struct _LP_PLAYBACK_MPM_OUTPUT {
    LP_PLAYBACK_U32 Version;
    LP_PLAYBACK_U32 SignalingGeneration;
    LP_PLAYBACK_U32 RequestSequence;
    LP_PLAYBACK_U32 StateFlags;
} LP_PLAYBACK_MPM_OUTPUT;
#pragma pack(pop)

static int LpPlaybackValidHeader(LP_PLAYBACK_U32 version, LP_PLAYBACK_U32 channel)
{
    return version == LP_PLAYBACK_VERSION && channel < LP_PLAYBACK_CHANNELS;
}

static int LpPlaybackValidPeer(LP_PLAYBACK_U64 address, LP_PLAYBACK_U64 owner, int paired)
{
    return paired && address != 0 && address <= 0xffffffffffffULL && address == owner;
}

static int LpPlaybackValidFlush(LP_PLAYBACK_U32 timeout)
{
    switch (timeout) {
    case 60: case 80: case 100: case 120: case 140: case 150: case 200: case 300:
        return 1;
    default:
        return 0;
    }
}

static int LpPlaybackValidSend(size_t bytes, LP_PLAYBACK_U32 mtu)
{
    return bytes != 0 && bytes <= LP_PLAYBACK_MAX_PACKET && mtu >= 48 && bytes <= mtu;
}
