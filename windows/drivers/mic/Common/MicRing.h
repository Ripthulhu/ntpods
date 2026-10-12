// PCM buffer shared by the kernel bridge and its user-mode regression harness.
#pragma once
#include <stddef.h>
#include <string.h>

#define MIC_PCM_MAX_WRITE 8192u
#define MIC_RING_BYTES 0x8000u
#define MIC_RING_HIGH_WATERMARK 0x3C00u
#define MIC_RING_TARGET_BYTES 0x2580u
// At 48 kHz PCM16, 100 ms leaves room for two packets plus one packet of jitter.
#define MIC_RT_MAX_PACKET_BYTES 9600u
static_assert(3u * MIC_RT_MAX_PACKET_BYTES <= MIC_RING_BYTES, "Capture packets exceed the PCM ring");

struct MicRing {
    unsigned char Data[MIC_RING_BYTES];
    unsigned Head;
    unsigned Tail;
    unsigned Count;
};

static inline bool MicPcmWriteLengthValid(size_t length)
{
    return length != 0 && length <= MIC_PCM_MAX_WRITE && (length & 1u) == 0;
}

static inline void MicRingFlush(MicRing* ring)
{
    ring->Head = ring->Tail = ring->Count = 0;
    memset(ring->Data, 0, sizeof(ring->Data));
}

// Caller serializes access. Each copy is bounded by the fixed ring capacity.
static inline bool MicRingWrite(MicRing* ring, const void* data, size_t length,
                                unsigned packetSize)
{
    if (!data || !MicPcmWriteLengthValid(length)) {
        return false;
    }
    const unsigned len = (unsigned)length;
    const unsigned free = MIC_RING_BYTES - ring->Count;
    if (len > free) {
        const unsigned drop = len - free;
        ring->Tail = (ring->Tail + drop) % MIC_RING_BYTES;
        ring->Count -= drop;
    }
    const unsigned first = len < MIC_RING_BYTES - ring->Head
        ? len : MIC_RING_BYTES - ring->Head;
    memcpy(ring->Data + ring->Head, data, first);
    memcpy(ring->Data, (const unsigned char*)data + first, len - first);
    ring->Head = (ring->Head + len) % MIC_RING_BYTES;
    ring->Count += len;

    unsigned target = MIC_RING_TARGET_BYTES;
    unsigned high = MIC_RING_HIGH_WATERMARK;
    // Clamp the adaptation before multiplying a framework-supplied packet size.
    if (packetSize != 0 && (packetSize & 1u) == 0 && packetSize <= MIC_RT_MAX_PACKET_BYTES) {
        if (target < 2u * packetSize) target = 2u * packetSize;
        if (high < target + packetSize) high = target + packetSize;
    }
    if (ring->Count > high) {
        const unsigned drop = ring->Count - target;
        ring->Tail = (ring->Tail + drop) % MIC_RING_BYTES;
        ring->Count = target;
    }
    return true;
}

// The caller zero-fills the remainder after releasing its spin lock.
static inline unsigned MicRingTake(MicRing* ring, void* out, unsigned length)
{
    if (!out || (length & 1u) != 0) return 0;
    const unsigned take = length < ring->Count ? length : ring->Count;
    const unsigned first = take < MIC_RING_BYTES - ring->Tail
        ? take : MIC_RING_BYTES - ring->Tail;
    memcpy(out, ring->Data + ring->Tail, first);
    memcpy((unsigned char*)out + first, ring->Data, take - first);
    ring->Tail = (ring->Tail + take) % MIC_RING_BYTES;
    ring->Count -= take;
    return take;
}
