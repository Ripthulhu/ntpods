// SPDX-License-Identifier: MIT
#define UNICODE
#define _UNICODE
#include <windows.h>
#include <setupapi.h>
#include <newdev.h>
#include <stdio.h>
#include <wchar.h>

static const wchar_t* const supported_ids[] = {
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

static bool supported_id(const wchar_t* id) {
    for (const wchar_t* supported : supported_ids) {
        if (_wcsicmp(id, supported) == 0) return true;
    }
    return false;
}

static int failed(const wchar_t* action) {
    DWORD code = GetLastError();
    fwprintf(stderr, L"%ls failed (Windows error %lu).\n", action, code);
    return 1;
}

int wmain(int argc, wchar_t** argv) {
    if (argc == 2 && wcscmp(argv[1], L"--self-test") == 0) {
        for (const wchar_t* id : supported_ids) {
            wchar_t lowercase[256] = {};
            wcscpy_s(lowercase, id);
            _wcslwr_s(lowercase);
            if (!supported_id(id) || !supported_id(lowercase)) return 1;
            wcscat_s(lowercase, L"extra");
            if (supported_id(lowercase)) return 1;
        }
        if (!supported_id(L"bthenum\\{0000110b-0000-1000-8000-00805f9b34fb}_vid&0001004c_pid&2027") ||
            !supported_id(L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&200E") ||
            supported_id(L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}") ||
            supported_id(L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2027extra") ||
            supported_id(L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&00010000_PID&2027") ||
            supported_id(L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0002004C_PID&2027") ||
            supported_id(L"BTHENUM\\{0000111e-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&2027") ||
            supported_id(L"BTHENUM\\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004C_PID&FFFF")) return 1;
        puts("Audio binding target checks passed.");
        return 0;
    }
    if (argc != 4 || (wcscmp(argv[1], L"--check") && wcscmp(argv[1], L"--bind")) ||
        wcslen(argv[3]) >= MAX_PATH) {
        fputs("Usage: audio-binding --check|--bind <device instance> <staged INF>\n", stderr);
        return 2;
    }
    HDEVINFO devices = SetupDiCreateDeviceInfoList(NULL, NULL);
    if (devices == INVALID_HANDLE_VALUE) return failed(L"Opening device list");
    SP_DEVINFO_DATA device = {};
    device.cbSize = sizeof(device);
    int result = 1;
    bool driver_list = false;
    do {
        if (!SetupDiOpenDeviceInfoW(devices, argv[2], NULL, 0, &device)) {
            result = failed(L"Opening the playback device"); break;
        }
        wchar_t ids[4096] = {};
        DWORD type = 0, required = 0;
        if (!SetupDiGetDeviceRegistryPropertyW(devices, &device, SPDRP_HARDWAREID,
                &type, reinterpret_cast<BYTE*>(ids), sizeof(ids), &required) ||
            type != REG_MULTI_SZ || required < 2 * sizeof(wchar_t) || required > sizeof(ids)) {
            result = failed(L"Reading playback hardware IDs"); break;
        }
        bool supported = false;
        // The final two characters are supplied locally even for a malformed property.
        ids[_countof(ids) - 1] = ids[_countof(ids) - 2] = 0;
        for (const wchar_t* id = ids; *id; id += wcslen(id) + 1) supported |= supported_id(id);
        if (!supported) { fputs("The device is not a supported AirPods AudioSink.\n", stderr); break; }

        SP_DEVINSTALL_PARAMS_W parameters = {};
        parameters.cbSize = sizeof(parameters);
        if (!SetupDiGetDeviceInstallParamsW(devices, &device, &parameters)) {
            result = failed(L"Reading driver installation parameters"); break;
        }
        parameters.Flags |= DI_ENUMSINGLEINF | DI_QUIETINSTALL;
        parameters.FlagsEx |= DI_FLAGSEX_ALLOWEXCLUDEDDRVS;
        wcscpy_s(parameters.DriverPath, argv[3]);
        if (!SetupDiSetDeviceInstallParamsW(devices, &device, &parameters) ||
            !SetupDiBuildDriverInfoList(devices, &device, SPDIT_COMPATDRIVER)) {
            result = failed(L"Finding a compatible driver in the requested INF"); break;
        }
        driver_list = true;
        SP_DRVINFO_DATA_W driver = {};
        driver.cbSize = sizeof(driver);
        if (!SetupDiEnumDriverInfoW(devices, &device, SPDIT_COMPATDRIVER, 0, &driver)) {
            result = failed(L"Selecting the compatible driver"); break;
        }
        if (wcscmp(argv[1], L"--check") == 0) { result = 0; break; }
        BOOL reboot = FALSE;
        // Supplying a device and driver prevents a broad hardware-ID update.
        if (!DiInstallDevice(NULL, devices, &device, &driver, 0, &reboot)) {
            result = failed(L"Installing the playback driver"); break;
        }
        result = reboot ? 3010 : 0;
    } while (false);
    if (driver_list) SetupDiDestroyDriverInfoList(devices, &device, SPDIT_COMPATDRIVER);
    SetupDiDestroyDeviceInfoList(devices);
    return result;
}
