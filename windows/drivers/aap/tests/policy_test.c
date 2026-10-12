#include <assert.h>
#include <stdio.h>
#include <stdint.h>
#include "../Policy.h"

static int valid(const wchar_t* value)
{
    return LpIsAllowedUserSid(value, wcslen(value));
}

int main(void)
{
    const wchar_t embedded[] = L"S-1-5-21-1-2-3-4\0)(A;;GA;;;WD)";
    assert(valid(L"S-1-5-21-1-2-3-1001"));
    assert(valid(L"S-1-5-21-4294967295-4294967295-4294967295-4294967295"));
    assert(!valid(L"S-1-5-21-4294967296-2-3-1001"));
    assert(!valid(L"S-1-5-21-1-2-3"));
    assert(!valid(L"S-1-5-21-1-2-3-4-5"));
    assert(!valid(L"S-1-5-21-01-2-3-4"));
    assert(!valid(L"S-1-5-21-1-2-3-"));
    assert(!valid(L"S-1-5-21-1-2-3--1"));
    assert(!valid(L"S-1-5-21-1-2-3-+1"));
    assert(!valid(L"S-1-5-21-1-2-3-4)(A;;GA;;;WD)"));
    assert(!valid(L"S-1-12-1-1-2-3-4"));
    assert(!valid(L"S-1-5-18"));
    assert(!valid(L"S-1-5-32-544"));
    assert(!valid(L""));
    assert(!LpIsAllowedUserSid(NULL, 20));
    assert(!LpIsAllowedUserSid(embedded, sizeof(embedded) / sizeof(*embedded) - 1));
    assert(LpReceiveTimeout(0) == 100);
    assert(LpReceiveTimeout(1) == 1);
    assert(LpReceiveTimeout(100) == 100);
    assert(LpReceiveTimeout(101) == 100);
    assert(LpReceiveTimeout(UINT32_MAX) == 100);
    assert(!LpValidPacketLength(0));
    assert(LpValidPacketLength(1));
    assert(LpValidPacketLength(8192));
    assert(!LpValidPacketLength(8193));
    assert(!LpValidPacketLength(SIZE_MAX));
    puts("AAP SID and I/O boundary tests passed");
    return 0;
}
