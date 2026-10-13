#pragma once

static_assert(sizeof(int) == 4, "Volume levels use signed 16.16 dB");
constexpr int RenderVolumeMinimum = -635 * 65536 / 10;
constexpr int RenderVolumeMaximum = 0;
constexpr unsigned RenderVolumeStep = 32768;
constexpr unsigned RenderVolumeLevelChanged = 1;
constexpr unsigned RenderVolumeMuteChanged = 2;

struct RenderVolumeState {
    int Level = RenderVolumeMinimum;
    bool Muted = false;
    bool Initialized = false;
    bool Subscribed = false;
};

inline unsigned RenderAssignVolume(RenderVolumeState* state, int level, bool remote)
{
    if (level < RenderVolumeMinimum) level = RenderVolumeMinimum;
    if (level > RenderVolumeMaximum) level = RenderVolumeMaximum;
    if (remote) state->Initialized = true;
    if (level == state->Level) return 0;
    const auto previousLevel = state->Level;
    const auto previousMute = state->Muted;
    // A remote mute echo must preserve the volume used by Windows to unmute.
    if (!remote || !state->Muted || level != RenderVolumeMinimum) state->Level = level;
    state->Muted = level == RenderVolumeMinimum;
    return (state->Level != previousLevel ? RenderVolumeLevelChanged : 0) |
        (state->Muted != previousMute ? RenderVolumeMuteChanged : 0);
}

inline unsigned RenderAssignMute(RenderVolumeState* state, bool muted)
{
    if (state->Muted == muted) return 0;
    state->Muted = muted;
    return RenderVolumeMuteChanged;
}

inline int RenderRemoteVolume(const RenderVolumeState* state)
{
    return state->Muted ? RenderVolumeMinimum : state->Level;
}

inline void RenderVolumeDisconnect(RenderVolumeState* state)
{
    state->Initialized = false;
    state->Subscribed = false;
}

inline bool RenderVolumeSilent(const RenderVolumeState* state)
{
    return !state->Initialized || !state->Subscribed || state->Muted ||
        state->Level == RenderVolumeMinimum;
}

inline bool RenderVolumeChannel(unsigned channel)
{
    return channel == ~0u || channel < 2;
}

inline bool RenderVolumeValueValid(unsigned controlBytes, unsigned valueBytes, const void* value)
{
    return controlBytes == 0 && valueBytes == sizeof(int) && value != nullptr;
}
