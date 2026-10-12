// SPDX-License-Identifier: MIT
#include <aacdecoder_lib.h>
#include <stdio.h>
#include <string.h>

static int read_u16(FILE *file) {
    unsigned char bytes[2];
    return fread(bytes, 1, 2, file) == 2 ? bytes[0] | bytes[1] << 8 : -1;
}

int main(int argc, char **argv) {
    if (argc != 3) { fprintf(stderr, "Usage: verify-eld encoded.bin decoded.f32\n"); return 1; }
    FILE *input = fopen(argv[1], "rb");
    FILE *output = fopen(argv[2], "wb");
    HANDLE_AACDECODER decoder = NULL;
    int result = 1, frames = 0;
    unsigned char magic[8], asc[64], au[320];
    INT_PCM pcm[4096];
    if (!input || !output || fread(magic, 1, 8, input) != 8 ||
        memcmp(magic, "NPELD001", 8) || read_u16(input) != 480) goto cleanup;
    int asc_size = read_u16(input);
    if (asc_size < 1 || asc_size > 64 ||
        fread(asc, 1, (size_t)asc_size, input) != (size_t)asc_size) goto cleanup;
    decoder = aacDecoder_Open(TT_MP4_RAW, 1);
    if (!decoder) goto cleanup;
    unsigned char *asc_pointer = asc;
    unsigned asc_length = (unsigned)asc_size;
    if (aacDecoder_ConfigRaw(decoder, &asc_pointer, &asc_length) != AAC_DEC_OK) goto cleanup;
    for (;;) {
        int first = fgetc(input);
        if (first == EOF) { if (ferror(input)) goto cleanup; break; }
        int second = fgetc(input);
        if (second == EOF) goto cleanup;
        int length = first | second << 8;
        if (length < 1 || length > 320 || fread(au, 1, (size_t)length, input) != (size_t)length) goto cleanup;
        unsigned char *au_pointer = au;
        unsigned au_length = (unsigned)length, remaining = au_length;
        if (aacDecoder_Fill(decoder, &au_pointer, &au_length, &remaining) != AAC_DEC_OK || remaining ||
            aacDecoder_DecodeFrame(decoder, pcm, 4096, 0) != AAC_DEC_OK) goto cleanup;
        CStreamInfo *info = aacDecoder_GetStreamInfo(decoder);
        if (!info || info->sampleRate != 48000 || info->numChannels != 2 || info->frameSize != 480) {
            fprintf(stderr, "Unexpected decoded audio format.\n");
            goto cleanup;
        }
        float interleaved[960];
        for (int i = 0; i < 960; ++i) interleaved[i] = pcm[i] / 32768.0f;
        if (fwrite(interleaved, 1, sizeof(interleaved), output) != sizeof(interleaved)) goto cleanup;
        ++frames;
    }
    printf("Decoded %d stereo frames at 48000 Hz, 480 samples per frame.\n", frames);
    result = 0;
cleanup:
    if (decoder) aacDecoder_Close(decoder);
    if (input) fclose(input);
    if (output && fclose(output)) result = 1;
    if (result) fprintf(stderr, "AAC-ELD decode verification failed.\n");
    return result;
}
