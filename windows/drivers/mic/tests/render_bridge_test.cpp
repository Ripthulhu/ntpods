#include "../Common/RenderPackets.h"
#include "../Common/RenderProvider.h"
#include "../Common/MicTiming.h"
#include "../Common/RenderClock.h"
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

static void finite_clock_checks()
{
    constexpr uint64_t epoch = 1234567890123ull;
    for (unsigned interval : {9u, 11u}) {
        RenderClock clock = {};
        RenderClockReset(&clock, epoch);
        assert(RenderClockNow(&clock, epoch) == epoch);
        assert(!RenderClockSet(&clock, epoch, 8, 40));
        assert(!RenderClockSet(&clock, epoch, interval, 0));
        assert(!RenderClockSet(&clock, epoch, interval, 501));
        assert(!RenderClockSet(&clock, epoch, 10, 1));
        assert(RenderClockSet(&clock, epoch, interval, 40));
        assert(clock.Phase == RenderClockArmed && clock.IntervalTenths == 10);
        const auto start = epoch + 100000;
        RenderClockCommit(&clock, start, 480);
        assert(clock.Phase == RenderClockActive && clock.ActualFrames == 0);
        assert(RenderClockNow(&clock, start) == start);
        uint64_t real = start;
        uint64_t previousPosition = 0;
        unsigned packets = 0;
        while (clock.Phase == RenderClockActive) {
            // Completion deadline and presentation use the very same map.
            const auto virtualDue = start + (uint64_t)(++packets) * 100000;
            const auto due = RenderClockDeadline(&clock, real, virtualDue);
            assert(due - real == interval * 10000);
            assert(RenderClockNow(&clock, due - 1) < virtualDue);
            assert(RenderClockNow(&clock, due) == virtualDue);
            const auto position = MicPositionBytes(0, RenderClockNow(&clock, due) - start, 0, 192000);
            assert(position >= previousPosition && position == (uint64_t)packets * 1920);
            previousPosition = position;
            real = due;
            RenderClockCommit(&clock, real, 480);
            assert(packets <= 40);
        }
        assert(packets == 40 && clock.Phase == RenderClockCompleted && clock.IntervalTenths == 10);
        assert(clock.ActualFrames == 19200);
        assert(clock.NominalFrames == (interval == 9 ? 17280u : 21120u));
        const auto value = RenderClockNow(&clock, real);
        assert(RenderClockNow(&clock, real + 10000000) == value + 10000000);
        assert(RenderClockSet(&clock, real + 10000000, 10, 0));
        assert(clock.Phase == RenderClockCancelled);
        assert(RenderClockNow(&clock, real + 10000000) == value + 10000000);

        // Cancellation and provider/generation reset preserve position, with
        // no finite adjustment surviving the new source generation.
        real += 20000000;
        assert(RenderClockSet(&clock, real, interval, 100));
        RenderClockCommit(&clock, real, 480);
        const auto beforeCancel = RenderClockNow(&clock, real + 300000);
        assert(RenderClockSet(&clock, real + 300000, 10, 0));
        assert(RenderClockNow(&clock, real + 300000) == beforeCancel);
        assert(RenderClockNow(&clock, real + 400000) == beforeCancel + 100000);
        assert(RenderClockSet(&clock, real + 400000, interval, 100));
        RenderClockCommit(&clock, real + 400000, 480);
        const auto beforeReset = RenderClockNow(&clock, real + 600000);
        RenderClockReset(&clock, real + 600000);
        assert(clock.Phase == RenderClockIdle && clock.IntervalTenths == 10);
        assert(clock.ActualFrames == 0 && clock.DeadlineReal == 0);
        assert(RenderClockNow(&clock, real + 600000) == beforeReset);
        assert(RenderClockNow(&clock, real + 700000) == beforeReset + 100000);
    }
}

static void clock_expiry_and_delay_checks()
{
    constexpr uint64_t epoch = 1234567890123ull;
    for (unsigned interval : {9u, 11u}) {
        RenderClock clock = {};
        RenderClockReset(&clock, epoch);
        assert(RenderClockSet(&clock, epoch, interval, 500));
        RenderClockCommit(&clock, epoch, 480);
        const auto expiry = epoch + 100000000;
        const auto expiryValue = RenderClockValue(&clock, expiry);
        // Timer inversion must account for the switch to nominal at expiry.
        assert(RenderClockDeadline(&clock, expiry - 100, expiryValue + 12345) == expiry + 12345);
        const auto before = RenderClockNow(&clock, expiry - 1);
        assert(clock.Phase == RenderClockActive);
        assert(RenderClockNow(&clock, expiry + 20000000) == expiryValue + 20000000);
        assert(clock.Phase == RenderClockExpired && clock.IntervalTenths == 10);
        assert(clock.ActualFrames == 0 && clock.NominalFrames == 480000);
        assert(expiryValue >= before && expiryValue - before <= 2);
        RenderClockCommit(&clock, expiry + 30000000, 480);
        assert(clock.Phase == RenderClockExpired && clock.ActualFrames == 0);

        RenderClockReset(&clock, expiry + 40000000);
        assert(RenderClockSet(&clock, expiry + 40000000, interval, 20));
        // No first packet: expiry is still local and finite, no speed change.
        const auto idleStart = RenderClockNow(&clock, expiry + 40000000);
        assert(RenderClockNow(&clock, expiry + 240000000) == idleStart + 200000000);
        assert(clock.Phase == RenderClockExpired && clock.ActualFrames == 0);
    }

    RenderClock clock = {};
    RenderClockReset(&clock, epoch);
    assert(RenderClockSet(&clock, epoch, 9, 10));
    RenderClockCommit(&clock, epoch, 480);
    // 100 ms elapsed would accrue >10 ms from the requested rate alone, but
    // only one actual packet arrived. It cannot count as a completed budget.
    assert(RenderClockNow(&clock, epoch + 1000000) - epoch > 1100000);
    assert(clock.Phase == RenderClockActive);
    RenderClockCommit(&clock, epoch + 1000000, 480);
    assert(clock.Phase == RenderClockActive && clock.ActualFrames == 480 && clock.NominalFrames == 4800);
    const auto virtualNow = RenderClockNow(&clock, epoch + 1000000);
    uint64_t glitch = 17;
    assert(RenderClockTimerDelay(&clock, epoch + 1000000, virtualNow - 300, &glitch) == -1);
    assert(glitch == 317);
    assert(RenderClockTimerDelay(&clock, epoch + 1000000, virtualNow + 100000, &glitch) < 0);
    assert(glitch == 317);

    // A late slow packet can fulfill a real frame deficit. Completion still
    // happens on that commit, not on a query that merely predicts the drift.
    RenderClockReset(&clock, epoch + 2000000);
    assert(RenderClockSet(&clock, epoch + 2000000, 11, 20));
    RenderClockCommit(&clock, epoch + 2000000, 480);
    (void)RenderClockNow(&clock, epoch + 2400000);
    assert(clock.Phase == RenderClockActive);
    const auto before = RenderClockNow(&clock, epoch + 2400000);
    RenderClockCommit(&clock, epoch + 2400000, 480);
    assert(clock.Phase == RenderClockCompleted && clock.ActualFrames == 480 && clock.NominalFrames == 1920);
    assert(RenderClockNow(&clock, epoch + 2400000) == before);

    RenderClockReset(&clock, epoch + 3000000);
    assert(RenderClockSet(&clock, epoch + 3000000, 9, 100));
    RenderClockCommit(&clock, epoch + 3000000, 480);
    const auto packetStart = RenderClockNow(&clock, epoch + 3000000);
    const auto armedDue = RenderClockDeadline(&clock, epoch + 3000000, packetStart + 100000);
    assert(RenderClockSet(&clock, epoch + 3045000, 10, 0));
    // An already armed fast timer is now early. The render callback rechecks
    // this deadline and rearms rather than publishing premature completion.
    assert(RenderClockNow(&clock, armedDue) < packetStart + 100000);
    glitch = 0;
    assert(RenderClockTimerDelay(&clock, armedDue, packetStart + 100000, &glitch) == -5000);
    assert(RenderClockNow(&clock, armedDue + 5000) == packetStart + 100000);
}

static void shared_clock_queue_checks()
{
    for (unsigned interval : {9u, 11u}) {
        RenderClock clock = {};
        RenderRing ring = {};
        uint32_t packet[480] = {}, output[480] = {};
        RenderClockReset(&clock, 0);
        uint64_t producerDue = 100000, consumerDue = 500000;
        unsigned produced = 0, consumed = 0;
        for (uint64_t real = 0; real <= 100000000; real += 10000) {
            if (real == 10000000) assert(RenderClockSet(&clock, real, interval, 100));
            const auto now = RenderClockNow(&clock, real);
            if (now >= producerDue) {
                std::fill(std::begin(packet), std::end(packet), ++produced);
                assert(RenderRingWrite(&ring, packet, sizeof(packet)));
                RenderClockCommit(&clock, real, 480);
                producerDue += 100000;
            }
            if (now >= consumerDue) {
                assert(RenderRingTake(&ring, output, sizeof(output)) == sizeof(output));
                ++consumed;
                assert(std::all_of(std::begin(output), std::end(output),
                    [consumed](auto frame) { return frame == consumed; }));
                consumerDue += 100000;
            }
            assert(ring.DroppedFrames == 0 && ring.Count <= 5 * sizeof(packet));
        }
        assert(clock.Phase == RenderClockCompleted && produced - consumed == 4);
    }
}

int main()
{
    ring_checks();
    packet_checks();
    pause_restart_generation_checks();
    stereo_clock_checks();
    provider_checks();
    finite_clock_checks();
    clock_expiry_and_delay_checks();
    shared_clock_queue_checks();
    std::puts("render bridge checks passed");
}
