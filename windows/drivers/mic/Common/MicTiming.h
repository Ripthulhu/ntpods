// Keep clock arithmetic shared with the portable regression checks.
#pragma once
#include "MicRing.h"

static inline bool MicRtPacketLayoutValid(unsigned count, unsigned size,
    unsigned maximum = MIC_RT_MAX_PACKET_BYTES)
{
    return count == 2 && size != 0 && size <= maximum && (size & 1u) == 0;
}

static inline unsigned long long MicPositionBytes(unsigned long long startPosition,
    unsigned long long elapsedHns, unsigned long long glitchHns, unsigned bytesPerSecond)
{
    // Glitch time is in 100 ns units, not bytes; subtract before converting.
    const auto activeHns = elapsedHns > glitchHns ? elapsedHns - glitchHns : 0;
    return startPosition + activeHns / 10000000u * bytesPerSecond
        + activeHns % 10000000u * bytesPerSecond / 10000000u;
}

static inline unsigned long long MicBytesToHns(unsigned long long bytes, unsigned bytesPerSecond)
{
    return bytes / bytesPerSecond * 10000000u
        + bytes % bytesPerSecond * 10000000u / bytesPerSecond;
}

static inline long long MicPacketTimerDelay(unsigned long long packetDueHns,
    unsigned long long nowHns, unsigned long long* glitchHns)
{
    if (packetDueHns <= nowHns) {
        *glitchHns += nowHns - packetDueHns;
        // Queue overdue work; calling the completion recursively can exhaust
        // the kernel stack when processing takes longer than a packet period.
        return -1;
    }
    return -(long long)(packetDueHns - nowHns);
}
