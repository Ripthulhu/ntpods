// Only canonical local/domain account SIDs are provisioned by this installer.
#pragma once
#include <stddef.h>

static inline bool MicAllowedUserSidValid(const wchar_t* sid, size_t length)
{
    const wchar_t prefix[] = L"S-1-5-21-";
    if (!sid || length < 15 || length > 68) return false;
    for (size_t i = 0; i < 9; ++i) {
        if (sid[i] != prefix[i]) return false;
    }
    size_t pos = 9;
    for (unsigned field = 0; field < 4; ++field) {
        const size_t first = pos;
        unsigned long long value = 0;
        while (pos < length && sid[pos] >= L'0' && sid[pos] <= L'9') {
            if (pos - first >= 10) return false;
            value = value * 10u + (unsigned)(sid[pos++] - L'0');
            if (value > 0xffffffffull) return false;
        }
        if (pos == first || (pos - first > 1 && sid[first] == L'0')) return false;
        if (field != 3) {
            if (pos == length || sid[pos++] != L'-') return false;
        }
    }
    return pos == length;
}
