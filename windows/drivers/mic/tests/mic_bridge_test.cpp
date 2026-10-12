#include "../Common/MicRequest.h"
#include "../Common/MicSid.h"
#include "../Common/MicTiming.h"
#include <algorithm>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cwchar>
#include <deque>
#include <limits>
#include <random>
#include <vector>

static void request_checks()
{
    using Kind = MicRequestKind;
    for (size_t length = 0; length <= MIC_PCM_MAX_WRITE + 8; ++length) {
        const auto expected = length > 0 && length <= MIC_PCM_MAX_WRITE && length % 2 == 0
            ? Kind::Write : Kind::BadSize;
        assert(MicValidateRequest(0x22a000, length, 0) == expected);
        assert(MicValidateRequest(0x22a000, length, 1) == Kind::BadSize);
    }
    assert(MicValidateRequest(0x22a000, std::numeric_limits<size_t>::max(), 0) == Kind::BadSize);
    assert(MicValidateRequest(0x22a000, size_t(1) << 32, 0) == Kind::BadSize);
    assert(MicValidateRequest(0x226004, 0, 3) == Kind::BadSize);
    assert(MicValidateRequest(0x226004, 0, 4) == Kind::Status);
    assert(MicValidateRequest(0x226004, 0, 8) == Kind::Status);
    assert(MicValidateRequest(0x226004, 1, 8) == Kind::BadSize);
    assert(MicValidateRequest(0x22a008, 0, 0) == Kind::Flush);
    assert(MicValidateRequest(0x22a008, 2, 0) == Kind::BadSize);
    assert(MicValidateRequest(0x22a008, 0, 4) == Kind::BadSize);
    assert(MicValidateRequest(0x22a00c, 0, 0) == Kind::Unknown);
}

static void sid_checks()
{
    for (const auto sid : {L"S-1-5-21-1-2-3-1001", L"S-1-5-21-4294967295-0-1-500"}) {
        assert(MicAllowedUserSidValid(sid, wcslen(sid)));
        assert(!MicAllowedUserSidValid(sid, wcslen(sid) + 1));
    }
    for (const auto sid : {L"", L"WD", L"AU", L"S-1-5-18", L"S-1-5-21-1-2-3",
             L"S-1-5-21-1-2-3-", L"S-1-5-21-1-2-3-4-5", L"S-1-5-21-01-2-3-4",
             L"S-1-5-21-4294967296-2-3-4", L"S-1-5-21-000000000000000-2-3-4",
             L"S-1-5-21-1-2-3-4)(A;;GA;;;WD)", L"S-1-5-21-1-2-3-4 ",
             L"S-1-12-1-1-2-3-4", L"s-1-5-21-1-2-3-4"}) {
        assert(!MicAllowedUserSidValid(sid, wcslen(sid)));
    }
    const wchar_t embedded[] = L"S-1-5-21-1-2-3-4\0evil";
    assert(!MicAllowedUserSidValid(embedded, sizeof(embedded) / sizeof(wchar_t) - 1));
    assert(!MicAllowedUserSidValid(nullptr, 30));
}

static void buffer_checks()
{
    struct Guarded {
        uint64_t before = 0x123456789abcdef0ull;
        MicRing ring = {};
        uint64_t after = 0xfedcba9876543210ull;
    } guarded;
    auto& ring = guarded.ring;
    std::deque<unsigned char> expected;
    std::mt19937 rng(4183);
    uint32_t sample = 0;
    for (unsigned step = 0; step < 20000; ++step) {
        if (step % 131 == 0) {
            MicRingFlush(&ring);
            expected.clear();
            assert(std::all_of(ring.Data, ring.Data + sizeof(ring.Data), [](auto x) { return x == 0; }));
        } else if (rng() % 3 != 0) {
            const unsigned bytes = 2 * (1 + rng() % (MIC_PCM_MAX_WRITE / 2));
            std::vector<unsigned char> input(bytes);
            for (unsigned i = 0; i < bytes; i += 2) {
                input[i] = (unsigned char)sample;
                input[i + 1] = (unsigned char)(sample++ >> 8);
            }
            assert(MicRingWrite(&ring, input.data(), input.size(), 480));
            expected.insert(expected.end(), input.begin(), input.end());
            if (expected.size() > MIC_RING_HIGH_WATERMARK) {
                while (expected.size() > MIC_RING_TARGET_BYTES) expected.pop_front();
            }
        } else {
            const unsigned bytes = 2 * (rng() % (MIC_RING_BYTES + 1));
            std::vector<unsigned char> out(bytes + 16, 0x55);
            const unsigned n = MicRingTake(&ring, out.data() + 8, bytes);
            assert(n == std::min<size_t>(bytes, expected.size()));
            for (unsigned i = 0; i < n; ++i) {
                assert(out[i + 8] == expected.front());
                expected.pop_front();
            }
            assert(std::all_of(out.begin(), out.begin() + 8, [](auto x) { return x == 0x55; }));
            assert(std::all_of(out.begin() + 8 + n, out.end(), [](auto x) { return x == 0x55; }));
        }
        assert(ring.Count == expected.size());
        assert((ring.Count | ring.Head | ring.Tail) % 2 == 0);
        assert(guarded.before == 0x123456789abcdef0ull);
        assert(guarded.after == 0xfedcba9876543210ull);
    }

    const auto before = ring;
    unsigned char bytes[MIC_PCM_MAX_WRITE + 2] = {};
    assert(!MicRingWrite(&ring, bytes, 1, 0));
    assert(!MicRingWrite(&ring, bytes, sizeof(bytes), 0));
    assert(!MicRingWrite(&ring, nullptr, 2, 0));
    assert(MicRingTake(&ring, bytes, 1) == 0);
    assert(memcmp(&ring, &before, sizeof(ring)) == 0);
    MicRingFlush(&ring);
    assert(MicRingTake(&ring, bytes, sizeof(bytes)) == 0);

    // A malicious/overflowing packet-size hint cannot overflow arithmetic or
    // disable the cap, and normal large capture packets retain their cushion.
    for (unsigned hint : {0u, 1u, 3840u, 9600u, 0xffffffffu}) {
        MicRingFlush(&ring);
        for (unsigned i = 0; i < 40; ++i) {
            assert(MicRingWrite(&ring, bytes, MIC_PCM_MAX_WRITE, hint));
            assert(ring.Count <= MIC_RING_BYTES);
            assert((ring.Count & 1u) == 0);
        }
        if (hint == 9600) assert(ring.Count >= 19200);
    }
}

static void timing_checks()
{
    constexpr unsigned bytesPerSecond = 48000 * 2;
    for (unsigned count : {0u, 1u, 2u, 3u, 0xffffffffu}) {
        for (unsigned size : {0u, 1u, 2u, 480u, 960u, 1920u, 4096u, 8192u,
                 MIC_RT_MAX_PACKET_BYTES, MIC_RT_MAX_PACKET_BYTES + 2, 19200u, 0xfffffffeu, 0xffffffffu}) {
            assert(MicRtPacketLayoutValid(count, size)
                == (count == 2 && size != 0 && size <= MIC_RT_MAX_PACKET_BYTES && size % 2 == 0));
        }
    }
    // Even a page-aligned single buffer requires an unsupported filling model.
    assert(!MicRtPacketLayoutValid(1, 4096));

    assert(MicPositionBytes(0, 100000, 0, bytesPerSecond) == 960);
    // A 10 ms timer stall after 20 ms must not subtract 100,000 bytes.
    assert(MicPositionBytes(0, 200000, 100000, bytesPerSecond) == 960);
    assert(MicPositionBytes(960, 200000, 100000, bytesPerSecond) == 1920);
    assert(MicPositionBytes(960, 100000, 200000, bytesPerSecond) == 960);

    // Thirty minutes at 10 ms packet cadence retain exact PCM frame positions.
    for (unsigned packet = 0; packet <= 180000; ++packet) {
        assert(MicPositionBytes(0, 100000ull * packet, 0, bytesPerSecond)
            == 960ull * packet);
    }
    // Multiplication before division overflowed after sufficiently long uptime.
    constexpr unsigned long long longHns = 250000000000000ull;
    assert(MicPositionBytes(0, longHns, 0, bytesPerSecond) == 2400000000000ull);
    assert(MicBytesToHns(960, bytesPerSecond) == 100000);
    constexpr unsigned long long wrappedPacket = 0x100000001ull;
    assert(MicBytesToHns(wrappedPacket * 960, bytesPerSecond) == wrappedPacket * 100000);
    assert(MicPositionBytes(0, MicBytesToHns(wrappedPacket * 960, bytesPerSecond), 0,
        bytesPerSecond) == wrappedPacket * 960);

    unsigned long long glitch = 0;
    assert(MicPacketTimerDelay(200000, 100000, &glitch) == -100000);
    assert(glitch == 0);
    assert(MicPacketTimerDelay(200000, 250000, &glitch) == -1);
    assert(glitch == 50000);
    assert(MicPacketTimerDelay(300000 + glitch, 250000, &glitch) == -100000);
    assert(glitch == 50000);
    assert(MicPacketTimerDelay(350000, 350000, &glitch) == -1);
    assert(glitch == 50000);
    // Even persistent overload produces one queued callback, never recursion.
    for (unsigned i = 0; i < 100000; ++i) {
        const auto prior = glitch;
        assert(MicPacketTimerDelay(prior + 100000, prior + 100002, &glitch) == -1);
        assert(glitch == prior + 2);
    }
}

static void capture_period_checks()
{
    // A 200 ms period used to pass allocation, then lose speech to ring trimming.
    assert(!MicRtPacketLayoutValid(2, 19200));
    assert(MicRtPacketLayoutValid(2, MIC_RT_MAX_PACKET_BYTES));
    MicRing ring = {};
    std::vector<unsigned char> packet(MIC_RT_MAX_PACKET_BYTES);
    unsigned char input[1440];
    unsigned produced = 0, delivered = 0;
    const auto write = [&] {
        for (unsigned i = 0; i < sizeof(input); ++i) input[i] = (unsigned char)(produced + i);
        assert(MicRingWrite(&ring, input, sizeof(input), MIC_RT_MAX_PACKET_BYTES));
        produced += sizeof(input);
    };
    // Prime 30 ms for the 15 ms producer/100 ms capture phase difference.
    write(); write();
    for (unsigned ms = 5; ms <= 3000; ms += 5) {
        if (ms % 15 == 0) write();
        if (ms % 100 == 0) {
            const unsigned taken = MicRingTake(&ring, packet.data(), (unsigned)packet.size());
            assert(taken == packet.size());
            for (unsigned i = 0; i < taken; ++i) assert(packet[i] == (unsigned char)(delivered + i));
            delivered += taken;
        }
        assert(ring.Count == produced - delivered);
    }
    assert(delivered == 3 * 48000 * 2);
}

int main()
{
    request_checks();
    sid_checks();
    buffer_checks();
    timing_checks();
    capture_period_checks();
    std::puts("mic bridge: request bounds, SID isolation, wrap/trim/flush, capture periods and timing checks passed");
}
