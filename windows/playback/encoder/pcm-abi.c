// SPDX-License-Identifier: MIT
#include <machine_type.h>

// Old 16-bit DLLs must fail to load rather than consume Q31 as S16.
__declspec(dllexport) int ntpods_fdk_pcm_bits(void) {
    return (int)sizeof(INT_PCM) * 8;
}
