// SPDX-License-Identifier: MIT
#include <aacenc_lib.h>
#include <fcntl.h>
#include <io.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

enum { CHANNELS = 2, FRAME_SAMPLES = 480, PCM_SAMPLES = 960 };

static int write_bytes(const void *data, size_t size) {
    return fwrite(data, 1, size, stdout) == size;
}

static int write_u16(unsigned value) {
    const unsigned char bytes[2] = {(unsigned char)value, (unsigned char)(value >> 8)};
    return write_bytes(bytes, sizeof(bytes));
}

static unsigned asc_bits(const unsigned char *asc, unsigned start, unsigned count) {
    unsigned result = 0;
    for (unsigned i = start; i < start + count; ++i) {
        result = (result << 1) | ((asc[i / 8] >> (7 - i % 8)) & 1);
    }
    return result;
}

static int valid_configuration(const AACENC_InfoStruct *info) {
    const unsigned char *asc = info->confBuf;
    // FDK accepts SBR_MODE even when no SBR module was allocated.
    return info->frameLength == FRAME_SAMPLES && info->inputChannels == CHANNELS &&
        info->confSize >= 4 && info->confSize <= 64 &&
        // Escape AOT 39, 48 kHz, stereo.
        asc_bits(asc, 0, 5) == 31 && asc_bits(asc, 5, 6) == 7 &&
        asc_bits(asc, 11, 4) == 3 && asc_bits(asc, 15, 4) == CHANNELS &&
        asc_bits(asc, 19, 1) == 1 && asc_bits(asc, 20, 3) == 0 &&
        // ELD SBR at the core rate, without CRC.
        asc_bits(asc, 23, 3) == 4;
}

static int encode(HANDLE_AACENCODER encoder, INT_PCM *pcm, int flushing, unsigned max_au) {
    unsigned char output[2048];
    void *input_ptr = pcm;
    void *output_ptr = output;
    int input_id = IN_AUDIO_DATA, output_id = OUT_BITSTREAM_DATA;
    int input_size = PCM_SAMPLES * (int)sizeof(INT_PCM);
    int output_size = (int)sizeof(output);
    int input_element = sizeof(INT_PCM), output_element = 1;
    AACENC_BufDesc input = {1, &input_ptr, &input_id, &input_size, &input_element};
    AACENC_BufDesc destination = {1, &output_ptr, &output_id, &output_size, &output_element};
    AACENC_InArgs in_args = {0};
    AACENC_OutArgs out_args = {0};
    in_args.numInSamples = flushing ? -1 : PCM_SAMPLES;
    AACENC_ERROR result = aacEncEncode(encoder, &input, &destination, &in_args, &out_args);
    if (result == AACENC_ENCODE_EOF) return 0;
    if (result != AACENC_OK || (!flushing && out_args.numInSamples != PCM_SAMPLES) ||
        out_args.numOutBytes < 0 || (unsigned)out_args.numOutBytes > max_au) {
        fprintf(stderr, "AAC-ELD encoding failed (%x, input %d, output %d).\n",
                result, out_args.numInSamples, out_args.numOutBytes);
        return -1;
    }
    if (out_args.numOutBytes &&
        (!write_u16((unsigned)out_args.numOutBytes) ||
         !write_bytes(output, (size_t)out_args.numOutBytes) || fflush(stdout))) {
        fprintf(stderr, "Cannot write encoded audio.\n");
        return -1;
    }
    return 1;
}

int main(int argc, char **argv) {
    HANDLE_AACENCODER encoder = NULL;
    AACENC_InfoStruct info = {0};
    INT_PCM pcm[PCM_SAMPLES];
    int exit_code = 1;
    unsigned bitrate = 256000;
    if (argc != 1) {
        if (argc != 3 || strcmp(argv[1], "--bitrate") || strlen(argv[2]) != 6 ||
            strspn(argv[2], "0123456789") != 6 || sscanf_s(argv[2], "%u", &bitrate) != 1 ||
            bitrate < 128000 || bitrate > 256000 || bitrate % 1000 != 0) {
            fprintf(stderr, "Use --bitrate with 128000..256000 bits per second in 1000-bit steps.\n");
            return 1;
        }
    }
    if (sizeof(INT_PCM) != 2 || _setmode(_fileno(stdin), _O_BINARY) == -1 ||
        _setmode(_fileno(stdout), _O_BINARY) == -1) {
        fprintf(stderr, "Cannot initialize PCM input and binary output.\n");
        return 1;
    }
    if (aacEncOpen(&encoder, 0x03, CHANNELS) != AACENC_OK) {
        fprintf(stderr, "Cannot open AAC-ELD encoder.\n");
        return 1;
    }
    const struct { AACENC_PARAM parameter; unsigned value; } parameters[] = {
        {AACENC_AOT, AOT_ER_AAC_ELD}, {AACENC_SAMPLERATE, 48000},
        {AACENC_CHANNELMODE, MODE_2}, {AACENC_GRANULE_LENGTH, FRAME_SAMPLES},
        {AACENC_SBR_MODE, 1}, {AACENC_SBR_RATIO, 1},
        {AACENC_BITRATE, bitrate}, {AACENC_BITRATEMODE, 5},
        // A 10 ms access unit must fit a whole-byte packet budget.
        {AACENC_PEAK_BITRATE, (bitrate / 800) * 800}, {AACENC_AFTERBURNER, 1},
        {AACENC_TRANSMUX, TT_MP4_RAW}
    };
    for (size_t i = 0; i < sizeof(parameters) / sizeof(parameters[0]); ++i) {
        if (aacEncoder_SetParam(encoder, parameters[i].parameter, parameters[i].value) != AACENC_OK) {
            fprintf(stderr, "Unsupported AAC-ELD parameter %x.\n", parameters[i].parameter);
            goto cleanup;
        }
    }
    if (aacEncEncode(encoder, NULL, NULL, NULL, NULL) != AACENC_OK ||
        aacEncInfo(encoder, &info) != AACENC_OK || !valid_configuration(&info)) {
        fprintf(stderr, "Cannot initialize the 480-sample AAC-ELD configuration.\n");
        goto cleanup;
    }
    if (!write_bytes("NPELD001", 8) || !write_u16(info.frameLength) ||
        !write_u16(info.confSize) || !write_bytes(info.confBuf, info.confSize) || fflush(stdout)) {
        fprintf(stderr, "Cannot write encoder configuration.\n");
        goto cleanup;
    }
    for (;;) {
        size_t count = fread(pcm, 1, sizeof(pcm), stdin);
        if (ferror(stdin) || (count && count != sizeof(pcm))) {
            fprintf(stderr, "PCM input must contain complete 480-frame stereo blocks.\n");
            goto cleanup;
        }
        if (!count) break;
        if (encode(encoder, pcm, 0, bitrate / 800) < 0) goto cleanup;
    }
    for (int i = 0; i < 32; ++i) {
        int result = encode(encoder, pcm, 1, bitrate / 800);
        if (result < 0) goto cleanup;
        if (!result) { exit_code = 0; goto cleanup; }
    }
    fprintf(stderr, "AAC-ELD flush exceeded its limit.\n");
cleanup:
    aacEncClose(&encoder);
    return exit_code;
}
