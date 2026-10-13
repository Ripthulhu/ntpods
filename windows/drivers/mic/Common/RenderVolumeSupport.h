#pragma once
#include "RenderVolume.h"

struct RenderVolumeSupport {
    KSPROPERTY_DESCRIPTION Description;
    KSPROPERTY_MEMBERSHEADER Members;
    KSPROPERTY_STEPPING_LONG Ranges[2];
};

inline RenderVolumeSupport RenderDescribeVolume(bool mute)
{
    RenderVolumeSupport support = {};
    support.Description.AccessFlags = KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT;
    support.Description.DescriptionSize = sizeof(support);
    support.Description.PropTypeSet.Set = {STATIC_KSPROPTYPESETID_General};
    support.Description.PropTypeSet.Id = mute ? VT_BOOL : VT_I4;
    support.Description.MembersListCount = 1;
    support.Members.MembersFlags = KSPROPERTY_MEMBER_STEPPEDRANGES;
    support.Members.MembersSize = sizeof(KSPROPERTY_STEPPING_LONG);
    support.Members.MembersCount = 2;
    // Both stereo channels share the headset's single absolute-volume control.
    support.Members.Flags = KSPROPERTY_MEMBER_FLAG_BASICSUPPORT_MULTICHANNEL |
        KSPROPERTY_MEMBER_FLAG_BASICSUPPORT_UNIFORM;
    for (auto& range : support.Ranges) {
        range.SteppingDelta = mute ? 1 : RenderVolumeStep;
        range.Bounds.SignedMinimum = mute ? 0 : RenderVolumeMinimum;
        range.Bounds.SignedMaximum = mute ? 1 : RenderVolumeMaximum;
    }
    return support;
}

inline unsigned RenderVolumeSupportBytes(unsigned capacity)
{
    if (capacity == sizeof(ULONG) || capacity == sizeof(KSPROPERTY_DESCRIPTION)) return capacity;
    return capacity >= sizeof(RenderVolumeSupport) ? sizeof(RenderVolumeSupport) : 0;
}
