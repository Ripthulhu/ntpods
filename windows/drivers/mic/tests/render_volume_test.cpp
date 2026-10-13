#define NOMINMAX
#include <windows.h>
#include <ks.h>
#include "../Common/RenderVolume.h"
#include "../Common/RenderVolumeSupport.h"
#include <assert.h>
#include <stdio.h>
#include <stdint.h>
#include <initializer_list>

int main()
{
    RenderVolumeState state;
    assert(RenderVolumeSilent(&state));
    RenderAssignVolume(&state, -10 * 65536, false);
    state.Subscribed = true;
    assert(RenderVolumeSilent(&state)); // Host writes do not initialize the service bridge.
    assert(RenderAssignVolume(&state, state.Level, true) == 0);
    assert(!RenderVolumeSilent(&state)); // Same-value first report still counts.
    assert(RenderAssignVolume(&state, state.Level, true) == 0); // Echo stops here.

    assert(RenderAssignMute(&state, true) == RenderVolumeMuteChanged);
    assert(state.Level == -10 * 65536 && RenderRemoteVolume(&state) == RenderVolumeMinimum);
    assert(RenderVolumeSilent(&state));
    assert(RenderAssignVolume(&state, RenderVolumeMinimum, true) == 0);
    assert(state.Level == -10 * 65536 && state.Muted);
    assert(RenderAssignVolume(&state, state.Level, true) == 0 && state.Muted);
    assert(RenderAssignMute(&state, false) == RenderVolumeMuteChanged);
    assert(RenderRemoteVolume(&state) == -10 * 65536 && !RenderVolumeSilent(&state));

    RenderAssignMute(&state, true);
    assert(RenderAssignVolume(&state, -12 * 65536 + 123, true) ==
        (RenderVolumeLevelChanged | RenderVolumeMuteChanged));
    assert(state.Level == -12 * 65536 + 123 && !state.Muted); // No half-dB rounding.
    RenderAssignMute(&state, true);
    assert(RenderAssignVolume(&state, RenderVolumeMinimum, false) == RenderVolumeLevelChanged);
    assert(state.Level == RenderVolumeMinimum && state.Muted);
    RenderAssignMute(&state, false);
    assert(RenderVolumeSilent(&state)); // Absolute-volume zero remains silent.

    RenderAssignVolume(&state, INT32_MAX, true);
    assert(state.Level == 0 && !state.Muted && !RenderVolumeSilent(&state));
    RenderAssignVolume(&state, INT32_MIN, true);
    assert(state.Level == RenderVolumeMinimum && state.Muted);
    RenderAssignVolume(&state, -5 * 65536, true);
    RenderVolumeDisconnect(&state);
    assert(RenderVolumeSilent(&state) && state.Level == -5 * 65536);
    state.Subscribed = true;
    assert(RenderVolumeSilent(&state)); // A new subscription needs a new report.
    RenderAssignVolume(&state, state.Level, true);
    assert(!RenderVolumeSilent(&state));
    RenderVolumeDisconnect(&state);
    RenderAssignVolume(&state, state.Level, true);
    assert(RenderVolumeSilent(&state)); // Report before subscription is also safe.
    state.Subscribed = true;
    assert(!RenderVolumeSilent(&state));
    assert(RenderVolumeChannel(UINT32_MAX) && RenderVolumeChannel(0) && RenderVolumeChannel(1));
    assert(!RenderVolumeChannel(2) && !RenderVolumeChannel(UINT32_MAX - 1));
    const int level = 0;
    assert(RenderVolumeValueValid(0, sizeof(level), &level));
    assert(!RenderVolumeValueValid(0, sizeof(level), nullptr));
    assert(!RenderVolumeValueValid(1, sizeof(level), &level));
    for (unsigned bytes = 0; bytes <= 8; ++bytes) {
        if (bytes != sizeof(level)) assert(!RenderVolumeValueValid(0, bytes, &level));
    }
    static_assert(sizeof(RenderVolumeSupport) == 88, "KS descriptor wire layout");
    for (bool mute : {false, true}) {
        const auto support = RenderDescribeVolume(mute);
        assert(support.Description.AccessFlags == (KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT));
        assert(support.Description.DescriptionSize == sizeof(support));
        assert(support.Description.PropTypeSet.Id == static_cast<ULONG>(mute ? VT_BOOL : VT_I4));
        assert(support.Description.MembersListCount == 1 && support.Description.Reserved == 0);
        assert(support.Members.Flags == 6 && support.Members.MembersCount == 2);
        assert(support.Members.MembersSize == sizeof(KSPROPERTY_STEPPING_LONG));
        for (const auto& range : support.Ranges) {
            assert(range.SteppingDelta == (mute ? 1u : RenderVolumeStep));
            assert(range.Bounds.SignedMinimum == (mute ? 0 : RenderVolumeMinimum));
            assert(range.Bounds.SignedMaximum == (mute ? 1 : RenderVolumeMaximum));
            assert(range.Reserved == 0);
        }
    }
    assert(RenderVolumeSupportBytes(4) == 4);
    assert(RenderVolumeSupportBytes(40) == 40);
    assert(RenderVolumeSupportBytes(88) == 88);
    assert(RenderVolumeSupportBytes(1000) == 88);
    for (unsigned capacity = 0; capacity < 88; ++capacity) {
        if (capacity != 4 && capacity != 40) assert(RenderVolumeSupportBytes(capacity) == 0);
    }
    puts("render volume tests passed");
}
