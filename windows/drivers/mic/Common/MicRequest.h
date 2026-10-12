#pragma once
#include "MicRing.h"

enum class MicRequestKind { Unknown, BadSize, Write, Status, Flush };

static inline MicRequestKind MicValidateRequest(unsigned code, size_t input, size_t output)
{
    switch (code) {
    case 0x0022A000u:
        return MicPcmWriteLengthValid(input) && output == 0
            ? MicRequestKind::Write : MicRequestKind::BadSize;
    case 0x00226004u:
        return input == 0 && output >= 4
            ? MicRequestKind::Status : MicRequestKind::BadSize;
    case 0x0022A008u:
        return input == 0 && output == 0
            ? MicRequestKind::Flush : MicRequestKind::BadSize;
    default:
        return MicRequestKind::Unknown;
    }
}
