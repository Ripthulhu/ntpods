#pragma once
#include "RenderRing.h"

struct RenderProvider {
    RenderRing Ring;
    bool Online, Open, Ready, Available;
    unsigned Streams;
};

inline bool RenderProviderConnected(const RenderProvider* state)
{
    return state->Online && state->Open && state->Available;
}

inline void RenderProviderOnline(RenderProvider* state, bool online)
{
    state->Online = online;
    state->Ready = state->Available = false;
    RenderRingFlush(&state->Ring);
}

inline void RenderProviderOpen(RenderProvider* state, bool open)
{
    state->Open = open;
    state->Ready = state->Available = false;
    RenderRingFlush(&state->Ring);
    if (open) state->Ring.DroppedFrames = 0;
}

inline bool RenderProviderReady(RenderProvider* state, unsigned ready)
{
    if (ready > 1 || (ready && (!state->Online || !state->Open))) return false;
    state->Ready = ready != 0;
    // A codec switch supplies silence without invalidating existing WASAPI streams.
    if (state->Ready) state->Available = true;
    RenderRingFlush(&state->Ring);
    return true;
}

inline void RenderProviderWrite(RenderProvider* state, const void* data, unsigned length)
{
    if (state->Online && state->Open && state->Ready && state->Streams)
        (void)RenderRingWrite(&state->Ring, data, length);
}
