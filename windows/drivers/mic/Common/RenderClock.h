// Render-only clock arithmetic, also exercised by the portable bridge tests.
#pragma once

enum RenderClockPhase : unsigned {
    RenderClockIdle = 0, RenderClockArmed = 1, RenderClockActive = 2,
    RenderClockCompleted = 3, RenderClockExpired = 4, RenderClockCancelled = 5
};

struct RenderClock {
    unsigned IntervalTenths, RequestedIntervalTenths, DeltaMs, Phase;
    unsigned long long RealOrigin, VirtualOrigin, StartReal, DeadlineReal;
    unsigned long long ActualFrames, NominalFrames;
};

static inline unsigned long long RenderClockScale(unsigned long long value,
    unsigned numerator, unsigned denominator)
{
    return value / denominator * numerator + value % denominator * numerator / denominator;
}

static inline unsigned long long RenderClockValue(const RenderClock* clock,
    unsigned long long real)
{
    const unsigned interval = clock->IntervalTenths ? clock->IntervalTenths : 10;
    // A QPC captured just before acquiring the context lock may precede a
    // concurrent transition. Keep that snapshot bounded by the new epoch.
    const auto elapsed = real > clock->RealOrigin ? real - clock->RealOrigin : 0;
    return clock->VirtualOrigin + RenderClockScale(elapsed, 10, interval);
}

static inline void RenderClockStop(RenderClock* clock, unsigned long long real, unsigned phase)
{
    clock->VirtualOrigin = RenderClockValue(clock, real);
    clock->RealOrigin = real;
    clock->IntervalTenths = 10;
    clock->Phase = phase;
}

static inline void RenderClockAdvance(RenderClock* clock, unsigned long long real)
{
    if ((clock->Phase == RenderClockArmed || clock->Phase == RenderClockActive)
        && real >= clock->DeadlineReal) {
        if (clock->Phase == RenderClockActive)
            clock->NominalFrames = RenderClockScale(clock->DeadlineReal - clock->StartReal, 48000, 10000000);
        // Restore at the deadline, even if the next query/packet arrives later.
        RenderClockStop(clock, clock->DeadlineReal, RenderClockExpired);
    }
}

static inline unsigned long long RenderClockNow(RenderClock* clock, unsigned long long real)
{
    RenderClockAdvance(clock, real);
    return RenderClockValue(clock, real);
}

static inline void RenderClockReset(RenderClock* clock, unsigned long long real)
{
    const auto value = RenderClockNow(clock, real);
    *clock = {};
    clock->IntervalTenths = clock->RequestedIntervalTenths = 10;
    clock->RealOrigin = real;
    clock->VirtualOrigin = value;
}

static inline bool RenderClockParametersValid(unsigned interval, unsigned deltaMs)
{
    return interval == 10 ? deltaMs == 0
        : ((interval == 9 || interval == 11) && deltaMs >= 1 && deltaMs <= 500);
}

static inline bool RenderClockSet(RenderClock* clock, unsigned long long real,
    unsigned interval, unsigned deltaMs)
{
    if (!RenderClockParametersValid(interval, deltaMs)) return false;
    RenderClockAdvance(clock, real);
    if (clock->Phase == RenderClockActive)
        clock->NominalFrames = RenderClockScale(real - clock->StartReal, 48000, 10000000);
    RenderClockStop(clock, real, RenderClockCancelled);
    if (interval == 10) return true;
    clock->RequestedIntervalTenths = interval;
    clock->DeltaMs = deltaMs;
    clock->ActualFrames = clock->NominalFrames = 0;
    clock->StartReal = 0;
    clock->DeadlineReal = real + 100000000ull; // local 10 second failsafe
    clock->Phase = RenderClockArmed;
    return true;
}

static inline void RenderClockCommit(RenderClock* clock, unsigned long long real, unsigned frames)
{
    RenderClockAdvance(clock, real);
    if (clock->Phase == RenderClockArmed) {
        // The just-finished packet ran at the old rate. Start counting at its
        // boundary, so the finite budget measures subsequent PCM completions.
        RenderClockStop(clock, real, RenderClockActive);
        clock->IntervalTenths = clock->RequestedIntervalTenths;
        clock->StartReal = real;
    } else if (clock->Phase == RenderClockActive) {
        clock->ActualFrames += frames;
        clock->NominalFrames = RenderClockScale(real - clock->StartReal, 48000, 10000000);
        const auto more = clock->IntervalTenths == 9 ? clock->ActualFrames : clock->NominalFrames;
        const auto less = clock->IntervalTenths == 9 ? clock->NominalFrames : clock->ActualFrames;
        if (more >= less && more - less >= (unsigned long long)clock->DeltaMs * 48)
            RenderClockStop(clock, real, RenderClockCompleted);
    }
}

static inline unsigned long long RenderClockDeadline(RenderClock* clock,
    unsigned long long realNow, unsigned long long virtualDue)
{
    const auto virtualNow = RenderClockNow(clock, realNow);
    if (virtualDue <= virtualNow) return realNow;
    if (clock->Phase == RenderClockActive) {
        const auto atExpiry = RenderClockValue(clock, clock->DeadlineReal);
        if (virtualDue > atExpiry)
            return clock->DeadlineReal + virtualDue - atExpiry;
    }
    const unsigned interval = clock->IntervalTenths ? clock->IntervalTenths : 10;
    // Invert from the epoch, rounding up, so fractional HNS cannot schedule a
    // completion before its presentation position. Do not accumulate rounding.
    const auto distance = virtualDue - clock->VirtualOrigin;
    const auto scaled = RenderClockScale(distance, interval, 10);
    return clock->RealOrigin + scaled + ((distance % 10) * interval % 10 != 0 ? 1 : 0);
}

static inline long long RenderClockTimerDelay(RenderClock* clock,
    unsigned long long realNow, unsigned long long virtualDue, unsigned long long* glitchHns)
{
    const auto virtualNow = RenderClockNow(clock, realNow);
    if (virtualDue <= virtualNow) {
        *glitchHns += virtualNow - virtualDue;
        return -1;
    }
    return -(long long)(RenderClockDeadline(clock, realNow, virtualDue) - realNow);
}
