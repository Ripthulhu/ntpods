// Stereo PCM shared by the ACX render bridge and portable tests.
#pragma once
#include <stddef.h>
#include <string.h>

#define RENDER_FRAME_BYTES 4u
#define RENDER_READ_MAX_BYTES 1920u
#define RENDER_PACKET_MAX_BYTES 9600u
#define RENDER_RING_BYTES 19200u

struct RenderRing {
    unsigned char Data[RENDER_RING_BYTES];
    unsigned Head, Tail, Count, DroppedFrames, Generation;
};

static inline bool RenderReadLengthValid(size_t length)
{
    return length != 0 && length <= RENDER_READ_MAX_BYTES && length % RENDER_FRAME_BYTES == 0;
}

static inline void RenderRingFlush(RenderRing* ring)
{
    ++ring->Generation;
    ring->Head = ring->Tail = ring->Count = 0;
    memset(ring->Data, 0, sizeof(ring->Data));
}

// Retain at most 100 ms; a stalled consumer never delays the Windows audio clock.
static inline bool RenderRingWrite(RenderRing* ring, const void* data, unsigned length)
{
    if (!data || !length || length > RENDER_PACKET_MAX_BYTES || length % RENDER_FRAME_BYTES) return false;
    const unsigned free = RENDER_RING_BYTES - ring->Count;
    if (length > free) {
        const unsigned drop = length - free;
        ring->Tail = (ring->Tail + drop) % RENDER_RING_BYTES;
        ring->Count -= drop;
        ring->DroppedFrames += drop / RENDER_FRAME_BYTES;
    }
    const unsigned first = length < RENDER_RING_BYTES - ring->Head
        ? length : RENDER_RING_BYTES - ring->Head;
    memcpy(ring->Data + ring->Head, data, first);
    memcpy(ring->Data, (const unsigned char*)data + first, length - first);
    ring->Head = (ring->Head + length) % RENDER_RING_BYTES;
    ring->Count += length;
    return true;
}

static inline unsigned RenderRingTake(RenderRing* ring, void* out, size_t length)
{
    if (!out || !RenderReadLengthValid(length)) return 0;
    const unsigned take = length < ring->Count ? (unsigned)length : ring->Count;
    const unsigned first = take < RENDER_RING_BYTES - ring->Tail ? take : RENDER_RING_BYTES - ring->Tail;
    memcpy(out, ring->Data + ring->Tail, first);
    memcpy((unsigned char*)out + first, ring->Data, take - first);
    ring->Tail = (ring->Tail + take) % RENDER_RING_BYTES;
    ring->Count -= take;
    return take;
}
