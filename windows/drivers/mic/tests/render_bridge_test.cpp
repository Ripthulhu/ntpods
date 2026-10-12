#include "../Common/RenderPackets.h"
#include "../Common/RenderProvider.h"
#include "../Common/MicTiming.h"
#include <algorithm>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <deque>
#include <limits>
#include <random>
#include <vector>

static void ring_checks()
{
    struct Guarded {
        uint64_t before = 0x1122334455667788ull;
        RenderRing ring = {};
        uint64_t after = 0x8877665544332211ull;
    } guarded;
    auto& ring = guarded.ring;
    std::deque<uint32_t> expected;
    std::mt19937 random(382);
    unsigned sample = 0, dropped = 0;
    for (unsigned step = 0; step < 20000; ++step) {
        if (step % 193 == 0) {
            RenderRingFlush(&ring);
            expected.clear();
            assert(std::all_of(ring.Data, ring.Data + sizeof(ring.Data), [](auto x) { return x == 0; }));
        } else if (random() % 3 != 0) {
            std::vector<uint32_t> data(1 + random() % (RENDER_PACKET_MAX_BYTES / 4));
            for (auto& frame : data) frame = ++sample;
            assert(RenderRingWrite(&ring, data.data(), (unsigned)data.size() * 4));
            expected.insert(expected.end(), data.begin(), data.end());
            while (expected.size() > RENDER_RING_BYTES / 4) { expected.pop_front(); ++dropped; }
        } else {
            std::vector<uint32_t> output(1 + random() % (RENDER_READ_MAX_BYTES / 4), 0xfefefefe);
            const unsigned count = RenderRingTake(&ring, output.data(), output.size() * 4) / 4;
            assert(count == (std::min)(expected.size(), output.size()));
            for (unsigned index = 0; index < count; ++index) {
                assert(output[index] == expected.front());
                expected.pop_front();
            }
            for (size_t index = count; index < output.size(); ++index) assert(output[index] == 0xfefefefe);
        }
        assert(ring.Count == expected.size() * 4);
        assert(ring.DroppedFrames == dropped);
        assert(guarded.before == 0x1122334455667788ull && guarded.after == 0x8877665544332211ull);
    }
    for (size_t length = 0; length <= RENDER_READ_MAX_BYTES + 8; ++length) {
        assert(RenderReadLengthValid(length) == (length != 0 && length <= RENDER_READ_MAX_BYTES && length % 4 == 0));
    }
    assert(!RenderReadLengthValid(std::numeric_limits<size_t>::max()));
    uint32_t output;
    assert(!RenderRingWrite(&ring, &output, 2));
    assert(!RenderRingWrite(&ring, &output, RENDER_PACKET_MAX_BYTES + 4));
    assert(RenderRingTake(&ring, &output, 3) == 0);
}

static void packet_checks()
{
    RenderPackets packets = {};
    using Result = RenderSubmitResult;
    assert(RenderSubmit(&packets, 0, false, 0, 0, 9999, 1920) == Result::Accepted);
    assert(RenderSubmit(&packets, 0, false, 2, 0, 0, 1920) == Result::Overrun);
    assert(RenderSubmit(&packets, 0, false, 1, 0x200, 480, 1920) == Result::Accepted);
    assert(RenderSubmit(&packets, 1, true, 2, 0, 0, 1920) == Result::Ended);
    assert(RenderPacketLength(&packets, 0, 1920) == 1920);
    assert(RenderPacketLength(&packets, 0, 1920) == 0);
    assert(RenderPacketLength(&packets, 1, 1920) == 480);
    assert(RenderPacketLength(&packets, 3, 1920) == 0);
    assert(RenderSubmit(&packets, 1, true, 2, 0, 0, 1920) == Result::Ended);
    RenderResetPackets(&packets);
    assert(RenderSubmit(&packets, 1, true, 1, 0, 0, 1920) == Result::Late);
    assert(RenderSubmit(&packets, 1, true, 0, 0, 0, 1920) == Result::Late);
    assert(RenderSubmit(&packets, 1, true, 2, 0x200, 0, 1920) == Result::Accepted);
    assert(RenderPacketLength(&packets, 2, 1920) == 0);
    assert(RenderSubmit(&packets, 2, true, 3, 0, 0, 1920) == Result::Ended);
    RenderResetPackets(&packets);
    assert(RenderSubmit(&packets, 1, true, 2, 0x200, 1921, 1920) == Result::Invalid);
    assert(RenderSubmit(&packets, 1, true, 2, 0x200, 2, 1920) == Result::Invalid);
    assert(RenderSubmit(&packets, 1, true, 2, 0x400, 0, 1920) == Result::Invalid);
    assert(RenderSubmit(&packets, 0xffffffffu, true, 0, 0, 0, 1920) == Result::Accepted);
    assert(RenderPacketLength(&packets, 0, 1920) == 1920);
    assert(RenderSubmit(&packets, 0, false, 0, 0, 0, 9600) == Result::Accepted);
    assert(RenderPacketLength(&packets, 0, 1920) == 0);
    assert(RenderSubmit(&packets, 0, false, 0, 0, 0, 9600) == Result::Accepted);
    RenderResetPackets(&packets);
    assert(RenderPacketLength(&packets, 0, 1920) == 0);
    assert(RenderSubmit(&packets, 0, false, 0, 0, 0, 1920) == Result::Accepted);
    assert(RenderPacketLength(&packets, 0, 1920) == 1920);
}

static void pause_restart_generation_checks()
{
    RenderRing ring = {};
    uint32_t oldPacket[480], newPacket[480], output[480];
    std::fill(std::begin(oldPacket), std::end(oldPacket), 0x11112222u);
    std::fill(std::begin(newPacket), std::end(newPacket), 0x33334444u);
    assert(RenderRingWrite(&ring, oldPacket, sizeof(oldPacket)));
    assert(RenderRingTake(&ring, output, 256) == 256);
    const unsigned observedGeneration = ring.Generation;
    const unsigned observedDrops = ring.DroppedFrames;

    // Last-client Pause and Run can both occur before the next status read.
    RenderRingFlush(&ring);
    RenderRingFlush(&ring);
    assert(RenderRingWrite(&ring, newPacket, sizeof(newPacket)));
    assert(ring.Generation == observedGeneration + 2);
    assert(ring.DroppedFrames == observedDrops);
    assert(RenderRingTake(&ring, output, sizeof(output)) == sizeof(newPacket));
    assert(std::equal(std::begin(newPacket), std::end(newPacket), std::begin(output)));

    ring.Generation = std::numeric_limits<unsigned>::max();
    RenderRingFlush(&ring);
    assert(ring.Generation == 0 && ring.Count == 0);
    RenderRingFlush(&ring);
    assert(ring.Generation == 1);
}

static void stereo_clock_checks()
{
    const unsigned bytesPerSecond = 48000 * 4;
    const unsigned packetBytes = 480 * 4;
    RenderRing ring = {};
    uint32_t packet[480], output[480];
    unsigned totalFrames = 0;
    for (unsigned tick = 0; tick < 6000; ++tick) {
        for (unsigned i = 0; i < 480; ++i) packet[i] = tick * 480 + i;
        assert(RenderRingWrite(&ring, packet, sizeof(packet)));
        assert(RenderRingTake(&ring, output, sizeof(output)) == packetBytes);
        assert(std::equal(std::begin(packet), std::end(packet), std::begin(output)));
        totalFrames += 480;
        const auto elapsed = MicBytesToHns((uint64_t)(tick + 1) * packetBytes, bytesPerSecond);
        assert(MicPositionBytes(0, elapsed, 0, bytesPerSecond) / 4 == totalFrames);
    }
    assert(totalFrames == 60 * 48000 && ring.Count == 0 && ring.DroppedFrames == 0);
    assert(MicBytesToHns(packetBytes, bytesPerSecond) == 100000);
    assert(MicPositionBytes(19200, 200000, 100000, bytesPerSecond) == 21120);
}

static void provider_checks()
{
    RenderProvider first = {}, second = {};
    const uint32_t sample = 0x1234abcd;
    uint32_t output = 0;
    RenderProviderOnline(&first, true);
    RenderProviderOnline(&second, true);
    first.Streams = second.Streams = 1;
    assert(!RenderProviderReady(&first, 1));
    RenderProviderOpen(&first, true);
    RenderProviderOpen(&second, true);
    RenderProviderWrite(&first, &sample, sizeof(sample));
    assert(!RenderProviderConnected(&first) && first.Ring.Count == 0);
    assert(!RenderProviderReady(&first, 2));
    assert(RenderProviderReady(&first, 1));
    assert(RenderProviderConnected(&first) && !RenderProviderConnected(&second));
    RenderProviderWrite(&first, &sample, sizeof(sample));
    assert(first.Ring.Count == sizeof(sample) && second.Ring.Count == 0);

    // Reconfiguring a codec clears old PCM while preserving the open endpoint.
    const unsigned generation = first.Ring.Generation;
    assert(RenderProviderReady(&first, 0));
    assert(RenderProviderConnected(&first) && first.Ring.Count == 0);
    assert(first.Ring.Generation == generation + 1);
    RenderProviderWrite(&first, &sample, sizeof(sample));
    assert(RenderRingTake(&first.Ring, &output, sizeof(output)) == 0);
    assert(RenderProviderReady(&first, 1));
    RenderProviderWrite(&first, &sample, sizeof(sample));
    assert(RenderRingTake(&first.Ring, &output, sizeof(output)) == sizeof(output));
    assert(output == sample);

    RenderProviderOpen(&first, false);
    assert(!RenderProviderConnected(&first) && !first.Ready && first.Ring.Count == 0);
    RenderProviderOpen(&first, true);
    assert(!RenderProviderConnected(&first));
    assert(RenderProviderReady(&first, 1));
    RenderProviderWrite(&first, &sample, sizeof(sample));
    RenderProviderOnline(&first, false);
    assert(!RenderProviderConnected(&first) && !first.Ready && first.Ring.Count == 0);
    assert(!RenderProviderReady(&first, 1));
    RenderProviderOnline(&first, true);
    assert(!RenderProviderConnected(&first));
    assert(RenderProviderReady(&first, 1));
    first.Streams = 0;
    RenderProviderWrite(&first, &sample, sizeof(sample));
    assert(first.Ring.Count == 0);
}

int main()
{
    ring_checks();
    packet_checks();
    pause_restart_generation_checks();
    stereo_clock_checks();
    provider_checks();
    std::puts("render bridge checks passed");
}
