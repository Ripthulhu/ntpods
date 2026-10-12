#pragma once

#include <stddef.h>
#include <wchar.h>

#define LP_MAX_PACKET_BYTES 8192u
#define LP_MAX_RECEIVE_MS 100u

/* Only a local/domain account SID may be interpolated into the device ACL. */
static int LpIsAllowedUserSid(const wchar_t* sid, size_t length)
{
    static const wchar_t prefix[] = L"S-1-5-21-";
    size_t pos;
    unsigned field;

    if (sid == NULL || length < 15 || length > 52) return 0;
    for (pos = 0; pos < 9; ++pos) {
        if (sid[pos] != prefix[pos]) return 0;
    }
    for (field = 0; field < 4; ++field) {
        unsigned long value = 0;
        size_t first = pos;
        while (pos < length && sid[pos] >= L'0' && sid[pos] <= L'9') {
            unsigned long digit = (unsigned long)(sid[pos] - L'0');
            if (value > (0xffffffffUL - digit) / 10u) return 0;
            value = value * 10u + digit;
            ++pos;
        }
        if (pos == first || pos - first > 10 ||
            (pos - first > 1 && sid[first] == L'0')) return 0;
        if (field < 3) {
            if (pos == length || sid[pos++] != L'-') return 0;
        }
    }
    return pos == length;
}

static unsigned long LpReceiveTimeout(unsigned long requested)
{
    return requested == 0 || requested > LP_MAX_RECEIVE_MS
        ? LP_MAX_RECEIVE_MS : requested;
}

static int LpValidPacketLength(size_t length)
{
    return length > 0 && length <= LP_MAX_PACKET_BYTES;
}
