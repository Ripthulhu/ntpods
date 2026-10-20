#pragma once
static_assert(sizeof(unsigned int) == 4 && sizeof(unsigned short) == 2,
    "PCM conversion requires 32-bit and 16-bit unsigned integers");

// Kernel render callbacks cannot use floating-point arithmetic. S16 / 32768
// is exactly representable as IEEE FLOAT32, including both integer extrema.
static inline unsigned int RenderS16FloatBits(unsigned short sample)
{
    if (!sample) return 0;
    const unsigned int sign = (unsigned int)(sample & 0x8000u) << 16;
    const unsigned int magnitude = sign ? 0x10000u - sample : sample;
    unsigned exponent = 0;
    for (unsigned int top = magnitude; top > 1; top >>= 1) ++exponent;
    return sign | ((exponent + 112u) << 23) |
        ((magnitude - (1u << exponent)) << (23u - exponent));
}
