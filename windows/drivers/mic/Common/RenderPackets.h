#pragma once
#include "RenderRing.h"

enum class RenderSubmitResult { Accepted, Invalid, Ended, Late, Overrun };

struct RenderPackets {
    unsigned Number[2];
    unsigned Length[2];
    bool Ready[2];
    bool EndOfStream;
};

static inline void RenderResetPackets(RenderPackets* packets)
{
    memset(packets, 0, sizeof(*packets));
}

static inline RenderSubmitResult RenderSubmit(RenderPackets* packets, unsigned current,
    bool running, unsigned packet, unsigned flags, unsigned eosLength, unsigned packetSize,
    unsigned sourceFrameBytes = 4)
{
    if (packets->EndOfStream) return RenderSubmitResult::Ended;
    if ((sourceFrameBytes != 4 && sourceFrameBytes != 8) || !packetSize ||
        packetSize % sourceFrameBytes || packetSize / sourceFrameBytes > RENDER_PACKET_MAX_BYTES / RENDER_FRAME_BYTES ||
        (flags & ~0x200u) || ((flags & 0x200u) && (eosLength > packetSize || eosLength % sourceFrameBytes))) {
        return RenderSubmitResult::Invalid;
    }
    const int delta = (int)(packet - current);
    if (delta < 0 || (running && delta == 0)) return RenderSubmitResult::Late;
    if (delta > 1) return RenderSubmitResult::Overrun;
    const unsigned index = packet % 2;
    packets->Number[index] = packet;
    packets->Length[index] = flags & 0x200u ? eosLength : packetSize;
    packets->Ready[index] = true;
    packets->EndOfStream = (flags & 0x200u) != 0;
    return RenderSubmitResult::Accepted;
}

// Consume the matching generation only; a missed packet must not replay old audio.
static inline unsigned RenderPacketLength(RenderPackets* packets, unsigned current, unsigned packetSize,
    unsigned sourceFrameBytes = 4)
{
    if (sourceFrameBytes != 4 && sourceFrameBytes != 8) return 0;
    const unsigned index = current % 2;
    if (!packets->Ready[index] || packets->Number[index] != current) return 0;
    packets->Ready[index] = false;
    const unsigned length = packets->Length[index];
    return length <= packetSize && length % sourceFrameBytes == 0 ? length : 0;
}
