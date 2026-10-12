#define UNICODE
#define _UNICODE
#define WIN32_LEAN_AND_MEAN
#define _WIN32_WINNT 0x0A00
#define NTDDI_VERSION 0x0A000002
#define USE_SP_ALTPLATFORM_INFO_V3 1
#include <windows.h>
#include <setupapi.h>
#include <cassert>
#include <cstdio>
#include <cwchar>

static bool selected(HINF inf, WORD architecture, DWORD build)
{
    INFCONTEXT manufacturer;
    assert(SetupFindFirstLineW(inf, L"Manufacturer", nullptr, &manufacturer));
    SP_ALTPLATFORM_INFO platform = {};
    platform.cbSize = sizeof(platform);
    platform.Platform = VER_PLATFORM_WIN32_NT;
    platform.ProcessorArchitecture = architecture;
    platform.MajorVersion = 10;
    platform.BuildNumber = build;
    platform.ProductType = VER_NT_WORKSTATION;
    wchar_t section[256] = {};
    return SetupDiGetActualModelsSectionW(&manufacturer, &platform, section, ARRAYSIZE(section), nullptr, nullptr)
        && SetupGetLineCountW(inf, section) > 0;
}

int wmain(int argc, wchar_t** argv)
{
    assert(argc == 2);
    HINF inf = SetupOpenInfFileW(argv[1], nullptr, INF_STYLE_WIN4 | INF_STYLE_CACHE_IGNORE, nullptr);
    assert(inf != INVALID_HANDLE_VALUE);
    assert(selected(inf, PROCESSOR_ARCHITECTURE_AMD64, 22000));
    assert(selected(inf, PROCESSOR_ARCHITECTURE_AMD64, 26200));
    assert(!selected(inf, PROCESSOR_ARCHITECTURE_AMD64, 21999));
    assert(!selected(inf, PROCESSOR_ARCHITECTURE_ARM64, 26200));
    assert(!selected(inf, PROCESSOR_ARCHITECTURE_INTEL, 26200));

    const wchar_t* models = L"Models.NTamd64.10.0...22000";
    const wchar_t* expected[] = {
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2002",
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&200A",
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&200E",
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&200F",
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2013",
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2014",
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2019",
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&201B",
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&201F",
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2024",
        L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2027"
    };
    assert(SetupGetLineCountW(inf, models) == static_cast<LONG>(ARRAYSIZE(expected)));
    for (unsigned index = 0; index < ARRAYSIZE(expected); ++index) {
        INFCONTEXT line;
        assert(SetupGetLineByIndexW(inf, models, index, &line));
        assert(SetupGetFieldCount(&line) == 2);
        wchar_t id[256] = {};
        assert(SetupGetStringFieldW(&line, 2, id, ARRAYSIZE(id), nullptr));
        assert(_wcsicmp(id, expected[index]) == 0);
    }
    INFCONTEXT service;
    assert(SetupFindFirstLineW(inf, L"Audio.NT.Services", L"AddService", &service));
    wchar_t name[64] = {};
    assert(SetupGetStringFieldW(&service, 1, name, ARRAYSIZE(name), nullptr));
    assert(wcscmp(name, L"NTPodsAudio") == 0);
    SetupCloseInfFile(inf);
    std::puts("Audio INF targets passed");
}
