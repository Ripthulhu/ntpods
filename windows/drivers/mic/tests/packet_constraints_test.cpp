#define NOMINMAX
#include <windows.h>
#include <ks.h>
#include <ksmedia.h>
#include <setupapi.h>
#include "../Common/PacketConstraints.h"
#include "../Common/MicTiming.h"
#include "../Common/RenderPackets.h"
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <cwchar>
#include <vector>

static void constraints_checks(ULONG alignment, ULONG maximum, unsigned frameBytes)
{
    const auto constraints = AudioPacketConstraints(alignment, maximum);
    static_assert(sizeof(constraints) == 40, "The published blob must contain its RAW entry");
    static_assert(offsetof(KSAUDIO_PACKETSIZE_CONSTRAINTS2, ProcessingModeConstraints) == 16);
    const GUID raw = {STATIC_AUDIO_SIGNALPROCESSINGMODE_RAW};
    assert(constraints.MinPacketPeriodInHns == 40000);
    assert(constraints.PacketSizeFileAlignment == alignment);
    assert(constraints.MaxPacketSizeInBytes == 9600 && maximum == 9600);
    assert(constraints.NumProcessingModeConstraints == 1);
    const auto& mode = constraints.ProcessingModeConstraints[0];
    assert(IsEqualGUID(mode.ProcessingMode, raw));
    assert(mode.SamplesPerProcessingPacket == 0 && mode.ProcessingPacketDurationInHns == 50000);
    const uint64_t byteTime = static_cast<uint64_t>(mode.ProcessingPacketDurationInHns) * 48000 * frameBytes;
    assert(byteTime % 10000000 == 0);
    const auto packetBytes = static_cast<unsigned>(byteTime / 10000000);
    assert(packetBytes == 240 * frameBytes);
    const auto period = MicBytesToHns(packetBytes, 48000 * frameBytes);
    assert(period == 50000 && period > constraints.MinPacketPeriodInHns);
    assert(packetBytes <= maximum);
    assert((packetBytes & alignment) == 0);
    // Every advertised quantum up to the transport cap fits the existing engine.
    for (unsigned bytes = packetBytes; bytes <= maximum; bytes += packetBytes) {
        assert(MicRtPacketLayoutValid(2, bytes));
    }
}

static void inf_constraints_checks(const wchar_t* path, const wchar_t* interfaces,
    const wchar_t* reference, ULONG alignment, ULONG maximum)
{
    HINF inf = SetupOpenInfFileW(path, nullptr, INF_STYLE_WIN4 | INF_STYLE_CACHE_IGNORE, nullptr);
    assert(inf != INVALID_HANDLE_VALUE);
    INFCONTEXT line;
    assert(SetupFindFirstLineW(inf, interfaces, L"AddInterface", &line));
    bool found = false;
    do {
        wchar_t category[64] = {};
        assert(SetupGetStringFieldW(&line, 1, category, ARRAYSIZE(category), nullptr));
        if (_wcsicmp(category, L"{6994AD04-93EF-11D0-A3CC-00A0C9223196}") != 0) continue;
        assert(!found);
        found = true;
        wchar_t name[64] = {}, section[128] = {};
        assert(SetupGetStringFieldW(&line, 2, name, ARRAYSIZE(name), nullptr));
        assert(wcscmp(name, reference) == 0);
        assert(SetupGetStringFieldW(&line, 3, section, ARRAYSIZE(section), nullptr));
        INFCONTEXT property;
        assert(SetupFindFirstLineW(inf, section, L"AddProperty", &property));
        assert(SetupGetFieldCount(&property) == 1);
        assert(SetupGetStringFieldW(&property, 1, section, ARRAYSIZE(section), nullptr));
        assert(SetupGetLineCountW(inf, section) == 1);
        assert(SetupFindFirstLineW(inf, section, nullptr, &property));
        wchar_t key[64] = {}, flags[16] = {};
        assert(SetupGetStringFieldW(&property, 1, key, ARRAYSIZE(key), nullptr));
        assert(_wcsicmp(key, L"{9404F781-7191-409B-8B0B-80BF6EC229AE}") == 0);
        int pid = 0, type = 0;
        assert(SetupGetIntField(&property, 2, &pid) && pid == 2);
        assert(SetupGetIntField(&property, 3, &type) && type == DEVPROP_TYPE_BINARY);
        assert(SetupGetStringFieldW(&property, 4, flags, ARRAYSIZE(flags), nullptr));
        assert(flags[0] == L'\0');
        BYTE bytes[40] = {};
        DWORD size = 0;
        assert(SetupGetFieldCount(&property) == 44);
        assert(SetupGetBinaryField(&property, 5, bytes, sizeof(bytes), &size));
        const auto expected = AudioPacketConstraints(alignment, maximum);
        assert(size == sizeof(expected) && memcmp(bytes, &expected, sizeof(expected)) == 0);
    } while (SetupFindNextMatchLineW(&line, L"AddInterface", &line));
    assert(found);
    SetupCloseInfFile(inf);
}

static void short_period_checks(unsigned milliseconds, unsigned frameBytes)
{
    const unsigned frames = milliseconds * 48;
    const unsigned bytes = frames * frameBytes;
    const unsigned bytesPerSecond = 48000 * frameBytes;
    const uint64_t period = milliseconds * 10000;
    assert(MicRtPacketLayoutValid(2, bytes));
    assert(!MicRtPacketLayoutValid(1, bytes));
    assert(MicBytesToHns(bytes, bytesPerSecond) == period);
    uint64_t glitch = 0;
    assert(MicPacketTimerDelay(period, 0, &glitch) == -static_cast<int64_t>(period));
    assert(glitch == 0);
    for (unsigned packet = 0; packet <= 30 * 60 * 1000 / milliseconds; ++packet) {
        const uint64_t position = static_cast<uint64_t>(packet) * bytes;
        assert(MicBytesToHns(position, bytesPerSecond) == packet * period);
        assert(MicPositionBytes(0, packet * period, 0, bytesPerSecond) == position);
    }
    // A delayed completion queues one pass and preserves the next full period.
    assert(MicPacketTimerDelay(period, period + 10000, &glitch) == -1);
    assert(glitch == 10000);
    assert(MicPacketTimerDelay(2 * period + glitch, period + 10000, &glitch) == -static_cast<int64_t>(period));

    std::vector<unsigned char> input(bytes), output(bytes);
    for (unsigned i = 0; i < bytes; ++i) input[i] = static_cast<unsigned char>(i);
    if (frameBytes == 4) {
        RenderRing ring = {};
        RenderPackets submitted = {};
        assert(RenderSubmit(&submitted, 0, false, 0, 0, 0, bytes) == RenderSubmitResult::Accepted);
        assert(RenderPacketLength(&submitted, 0, bytes) == bytes);
        assert(RenderRingWrite(&ring, input.data(), bytes));
        assert(RenderRingTake(&ring, output.data(), bytes) == bytes);
        assert(ring.Count == 0 && ring.DroppedFrames == 0);
    } else {
        MicRing ring = {};
        assert(MicRingWrite(&ring, input.data(), bytes, bytes));
        assert(MicRingTake(&ring, output.data(), bytes) == bytes);
        assert(ring.Count == 0);
    }
    assert(input == output);
}

int wmain(int argc, wchar_t** argv)
{
    assert(argc == 3);
    constraints_checks(3u, RENDER_PACKET_MAX_BYTES, 4); // FILE_LONG_ALIGNMENT
    constraints_checks(1u, MIC_RT_MAX_PACKET_BYTES, 2); // FILE_WORD_ALIGNMENT
    inf_constraints_checks(argv[1], L"Audio.NT.Interfaces", L"Headphones0", 3u, RENDER_PACKET_MAX_BYTES);
    inf_constraints_checks(argv[2], L"Audio_Device.NT.Interfaces", L"Microphone0", 1u, MIC_RT_MAX_PACKET_BYTES);
    for (unsigned milliseconds : {4u, 5u, 10u}) {
        short_period_checks(milliseconds, 4);
        short_period_checks(milliseconds, 2);
    }
    std::puts("ACX constraints: RAW 5 ms runtime and INF blobs, packet timing and PCM delivery passed");
}
