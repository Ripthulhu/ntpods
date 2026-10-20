#pragma once
#include "RenderRing.h"
#include "RenderVolume.h"

constexpr unsigned RenderDiagnosticInitialized = 1;
constexpr unsigned RenderDiagnosticSubscribed = 2;
constexpr unsigned RenderDiagnosticMuted = 4;
constexpr unsigned RenderDiagnosticMinimum = 8;

// Read-only version 1. Times use QPC-derived 100 ns units; zero means unobserved.
struct RenderDiagnostics {
    unsigned Version, Generation, VolumeFlags;
    int Level;
    unsigned long long FirstPacketHns, InitializedHns, SubscribedHns;
    unsigned long long FirstSourceNonzeroHns, FirstForwardedNonzeroHns;
    unsigned long long Packets, MissingPackets, SourceZeroPackets, SourceNonzeroPackets;
    unsigned long long GatedPackets, UninitializedPackets, UnsubscribedPackets;
    unsigned long long MutedPackets, MinimumPackets;
};
static_assert(sizeof(RenderDiagnostics) == 128, "render diagnostics ABI");

inline unsigned RenderDiagnosticVolumeFlags(const RenderVolumeState* volume)
{
    return (volume->Initialized ? RenderDiagnosticInitialized : 0u) |
        (volume->Subscribed ? RenderDiagnosticSubscribed : 0u) |
        (volume->Muted ? RenderDiagnosticMuted : 0u) |
        (volume->Level == RenderVolumeMinimum ? RenderDiagnosticMinimum : 0u);
}

inline void RenderDiagnosticVolume(RenderDiagnostics* state, const RenderVolumeState* volume,
    unsigned long long now)
{
    const auto flags = RenderDiagnosticVolumeFlags(volume);
    const auto tick = now ? now : 1;
    if (!(flags & RenderDiagnosticInitialized)) state->InitializedHns = 0;
    else if (!(state->VolumeFlags & RenderDiagnosticInitialized)) state->InitializedHns = tick;
    if (!(flags & RenderDiagnosticSubscribed)) state->SubscribedHns = 0;
    else if (!(state->VolumeFlags & RenderDiagnosticSubscribed)) state->SubscribedHns = tick;
    state->VolumeFlags = flags;
    state->Level = volume->Level;
}

inline void RenderDiagnosticReset(RenderDiagnostics* state, unsigned generation)
{
    const auto flags = state->VolumeFlags;
    const auto level = state->Level;
    const auto initialized = state->InitializedHns;
    const auto subscribed = state->SubscribedHns;
    *state = {};
    state->Version = 1;
    state->Generation = generation;
    state->VolumeFlags = flags;
    state->Level = level;
    state->InitializedHns = initialized;
    state->SubscribedHns = subscribed;
}

inline bool RenderDiagnosticNonzero(const void* data, unsigned length, unsigned frameBytes)
{
    if (!data || (frameBytes != 4 && frameBytes != 8) || length % frameBytes ||
        length / frameBytes > RENDER_PACKET_MAX_BYTES / RENDER_FRAME_BYTES) return false;
    const auto bytes = static_cast<const unsigned char*>(data);
    if (frameBytes == 4) {
        for (unsigned index = 0; index < length; ++index) if (bytes[index]) return true;
    } else {
        // Inspect IEEE FLOAT32 bits without using floating point at DISPATCH_LEVEL.
        for (unsigned index = 0; index < length; index += 4) {
            if (bytes[index] || bytes[index + 1] || bytes[index + 2] || (bytes[index + 3] & 0x7fu)) return true;
        }
    }
    return false;
}

inline void RenderDiagnosticIncrement(unsigned long long* value)
{
    if (*value != ~0ull) ++*value;
}

inline void RenderDiagnosticPacket(RenderDiagnostics* state, unsigned long long now,
    unsigned flags, bool missing, unsigned submitted, bool nonzero)
{
    const auto tick = now ? now : 1;
    if (!state->FirstPacketHns) state->FirstPacketHns = tick;
    RenderDiagnosticIncrement(&state->Packets);
    if (missing) RenderDiagnosticIncrement(&state->MissingPackets);
    else if (submitted) {
        RenderDiagnosticIncrement(nonzero ? &state->SourceNonzeroPackets : &state->SourceZeroPackets);
        if (nonzero && !state->FirstSourceNonzeroHns) state->FirstSourceNonzeroHns = tick;
    }
    if (!(flags & RenderDiagnosticInitialized)) RenderDiagnosticIncrement(&state->UninitializedPackets);
    if (!(flags & RenderDiagnosticSubscribed)) RenderDiagnosticIncrement(&state->UnsubscribedPackets);
    if (flags & RenderDiagnosticMuted) RenderDiagnosticIncrement(&state->MutedPackets);
    if (flags & RenderDiagnosticMinimum) RenderDiagnosticIncrement(&state->MinimumPackets);
    if (flags != (RenderDiagnosticInitialized | RenderDiagnosticSubscribed)) {
        RenderDiagnosticIncrement(&state->GatedPackets);
    } else if (!missing && submitted && nonzero && !state->FirstForwardedNonzeroHns) {
        state->FirstForwardedNonzeroHns = tick;
    }
}
