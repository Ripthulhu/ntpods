// Read-only regression test using Windows' actual INF model selection.
#define UNICODE
#define _UNICODE
#define WIN32_LEAN_AND_MEAN
#define _WIN32_WINNT 0x0A00
#define NTDDI_VERSION 0x0A000002
#define USE_SP_ALTPLATFORM_INFO_V3 1
#include <windows.h>
#include <setupapi.h>
#include <stdio.h>
#include <wchar.h>

static int CheckTarget(HINF inf, INFCONTEXT* manufacturer, WORD architecture,
                       DWORD major, DWORD minor, DWORD build, BYTE product,
                       BOOL expected, const wchar_t* label)
{
    SP_ALTPLATFORM_INFO platform = { 0 };
    wchar_t section[256] = { 0 };
    BOOL selected;
    platform.cbSize = sizeof(platform);
    platform.Platform = VER_PLATFORM_WIN32_NT;
    platform.ProcessorArchitecture = architecture;
    platform.MajorVersion = major;
    platform.MinorVersion = minor;
    platform.BuildNumber = build;
    platform.ProductType = product;
    selected = SetupDiGetActualModelsSectionW(manufacturer, &platform, section,
                                              ARRAYSIZE(section), NULL, NULL);
    // A fallback name without a populated Models section is not installable.
    selected = selected && SetupGetLineCountW(inf, section) > 0;
    if (selected != expected) {
        fwprintf(stderr, L"FAIL %ls: matched=%d expected=%d section=%ls\n",
                 label, selected, expected, section);
        return 1;
    }
    wprintf(L"PASS %ls\n", label);
    return 0;
}

int wmain(int argc, wchar_t** argv)
{
    HINF inf;
    INFCONTEXT manufacturer;
    int failures = 0;
    if (argc != 2) {
        fwprintf(stderr, L"Usage: inf_target_test NTPodsAAP.inf\n");
        return 2;
    }
    inf = SetupOpenInfFileW(argv[1], NULL,
                           INF_STYLE_WIN4 | INF_STYLE_CACHE_IGNORE, NULL);
    if (inf == INVALID_HANDLE_VALUE) {
        fwprintf(stderr, L"Cannot read INF: %lu\n", GetLastError());
        return 2;
    }
    if (!SetupFindFirstLineW(inf, L"Manufacturer", NULL, &manufacturer)) {
        fwprintf(stderr, L"Missing Manufacturer entry: %lu\n", GetLastError());
        SetupCloseInfFile(inf);
        return 2;
    }
    failures += CheckTarget(inf, &manufacturer, PROCESSOR_ARCHITECTURE_AMD64,
        10, 0, 22000, VER_NT_WORKSTATION, TRUE, L"Windows 11 workstation");
    failures += CheckTarget(inf, &manufacturer, PROCESSOR_ARCHITECTURE_AMD64,
        10, 0, 26200, VER_NT_WORKSTATION, TRUE, L"Windows 11 26200 workstation");
    failures += CheckTarget(inf, &manufacturer, PROCESSOR_ARCHITECTURE_AMD64,
        10, 0, 26200, VER_NT_SERVER, TRUE, L"Windows 11 26200 multi-session product type");
    failures += CheckTarget(inf, &manufacturer, PROCESSOR_ARCHITECTURE_AMD64,
        10, 0, 16299, VER_NT_WORKSTATION, TRUE, L"Existing minimum Windows build");
    failures += CheckTarget(inf, &manufacturer, PROCESSOR_ARCHITECTURE_AMD64,
        10, 0, 16298, VER_NT_WORKSTATION, FALSE, L"Below minimum build rejected");
    failures += CheckTarget(inf, &manufacturer, PROCESSOR_ARCHITECTURE_AMD64,
        6, 3, 9600, VER_NT_WORKSTATION, FALSE, L"Old Windows version rejected");
    failures += CheckTarget(inf, &manufacturer, PROCESSOR_ARCHITECTURE_ARM64,
        10, 0, 26200, VER_NT_WORKSTATION, FALSE, L"ARM64 rejected");
    failures += CheckTarget(inf, &manufacturer, PROCESSOR_ARCHITECTURE_INTEL,
        10, 0, 26200, VER_NT_WORKSTATION, FALSE, L"x86 rejected");
    SetupCloseInfFile(inf);
    return failures ? 1 : 0;
}
