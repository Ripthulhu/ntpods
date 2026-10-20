#include "NTPodsAAP.h"
#include <aux_klib.h>
#include <ntimage.h>

#if NTPODS_EXPERIMENTAL_PLAYBACK

typedef NTSTATUS (NTAPI *LP_QUERY_FEATURE)(ULONG, ULONG, PULONGLONG, PVOID);

BOOLEAN LpMpmSupported(VOID)
{
    RTL_OSVERSIONINFOW version = { sizeof(version) };
    UNICODE_STRING name = RTL_CONSTANT_STRING(L"RtlQueryFeatureConfiguration");
    LP_QUERY_FEATURE query;
    AUX_MODULE_EXTENDED_INFO* modules;
    ULONG bytes = 0, count, index;
    BOOLEAN matched = FALSE;
    ULONGLONG stamp = 0;
    ULONG configuration[3] = {0};
    NTSTATUS status;
    if (!NT_SUCCESS(RtlGetVersion(&version)) ||
        (version.dwBuildNumber != 26100 && version.dwBuildNumber != 26200))
        return FALSE;
    query = (LP_QUERY_FEATURE)MmGetSystemRoutineAddress(&name);
    if (!query || !NT_SUCCESS(AuxKlibInitialize())) return FALSE;
    status = AuxKlibQueryModuleInformation(&bytes, sizeof(*modules), NULL);
    if (!NT_SUCCESS(status) || !bytes || bytes > 1024 * 1024) return FALSE;
    modules = ExAllocatePool2(POOL_FLAG_PAGED, bytes, LP_POOL_TAG);
    if (!modules) return FALSE;
    status = AuxKlibQueryModuleInformation(&bytes, sizeof(*modules), modules);
    if (NT_SUCCESS(status)) {
        count = bytes / sizeof(*modules);
        for (index = 0; index < count; ++index) {
            PUCHAR base = modules[index].BasicInfo.ImageBase;
            ULONG size = modules[index].ImageSize;
            ULONG_PTR address = (ULONG_PTR)BtaMpmRegister;
            if (address < (ULONG_PTR)base || address - (ULONG_PTR)base >= size)
                continue;
            /* Loaded btampm 10.0.26100.9444, matching PDB DA64939B.../1.
             * Inspect mapped headers, not a possibly newer file on disk. */
            __try {
                PIMAGE_DOS_HEADER dos = (PIMAGE_DOS_HEADER)base;
                PIMAGE_NT_HEADERS64 nt;
                if (size < sizeof(*nt) || dos->e_magic != IMAGE_DOS_SIGNATURE ||
                    dos->e_lfanew < 0 || (ULONG)dos->e_lfanew > size - sizeof(*nt))
                    break;
                nt = (PIMAGE_NT_HEADERS64)(base + dos->e_lfanew);
                matched = nt->Signature == IMAGE_NT_SIGNATURE &&
                    nt->FileHeader.Machine == IMAGE_FILE_MACHINE_AMD64 &&
                    nt->OptionalHeader.Magic == IMAGE_NT_OPTIONAL_HDR64_MAGIC &&
                    nt->FileHeader.TimeDateStamp == 0xf7e78ad1 &&
                    nt->OptionalHeader.SizeOfImage == 0x15000 &&
                    nt->OptionalHeader.CheckSum == 0x17fdc;
            } __except (EXCEPTION_EXECUTE_HANDLER) {
                matched = FALSE;
            }
            break;
        }
    }
    ExFreePoolWithTag(modules, LP_POOL_TAG);
    if (!matched) return FALSE;
    /* Feature symbol hash 3803415865 maps to ID 62779942. Stage 3 uses
     * Boot configuration (0); bits 4..5 must explicitly say Enabled (2). */
    status = query(62779942, 0, &stamp, configuration);
    return status == STATUS_SUCCESS && ((configuration[1] >> 4) & 3) == 2;
}

#endif
