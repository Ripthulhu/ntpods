// SPDX-License-Identifier: MIT
#include <aacenc_lib.h>
#include <fcntl.h>
#include <io.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <windows.h>
#include <avrt.h>

enum { CHANNELS = 2, FRAME_SAMPLES = 480, PCM_SAMPLES = 960 };

static int write_bytes(const void *data, size_t size) {
    return fwrite(data, 1, size, stdout) == size;
}

static int write_u16(unsigned value) {
    const unsigned char bytes[2] = {(unsigned char)value, (unsigned char)(value >> 8)};
    return write_bytes(bytes, sizeof(bytes));
}

static int valid_configuration(const AACENC_InfoStruct *info) {
    // AOT 39, 48 kHz stereo, 480 samples, no SBR or error-protection extensions.
    const unsigned char expected[] = {0xf8, 0xe6, 0x50, 0x00};
    return info->frameLength == FRAME_SAMPLES && info->inputChannels == CHANNELS &&
        info->confSize == sizeof(expected) && !memcmp(info->confBuf, expected, sizeof(expected));
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
    DWORD task_index = 0;
    HANDLE audio_task = NULL;
    unsigned bitrate = 256000;
    const int variable = argc == 2 && !strcmp(argv[1], "--vbr");
    if (variable) {
        bitrate = 192000;
    } else if (argc != 1) {
        if (argc != 3 || strcmp(argv[1], "--bitrate") || strlen(argv[2]) != 6 ||
            strspn(argv[2], "0123456789") != 6 || sscanf_s(argv[2], "%u", &bitrate) != 1 ||
            bitrate < 128000 || bitrate > 256000 || bitrate % 1000 != 0) {
            fprintf(stderr, "Use --bitrate with 128000..256000 bits per second in 1000-bit steps, or --vbr.\n");
            return 1;
        }
    }
    const unsigned peak_bitrate = variable ? 256000 : bitrate;
    if (sizeof(INT_PCM) != 2 || _setmode(_fileno(stdin), _O_BINARY) == -1 ||
        _setmode(_fileno(stdout), _O_BINARY) == -1) {
        fprintf(stderr, "Cannot initialize PCM input and binary output.\n");
        return 1;
    }
    if (aacEncOpen(&encoder, 0x01, CHANNELS) != AACENC_OK) {
        fprintf(stderr, "Cannot open AAC-ELD encoder.\n");
        return 1;
    }
    const struct { AACENC_PARAM parameter; unsigned value; } parameters[] = {
        {AACENC_AOT, AOT_ER_AAC_ELD}, {AACENC_SAMPLERATE, 48000},
        {AACENC_CHANNELMODE, MODE_2}, {AACENC_GRANULE_LENGTH, FRAME_SAMPLES},
        {AACENC_SBR_MODE, 0},
        {AACENC_BITRATE, bitrate}, {AACENC_BITRATEMODE, variable ? 5 : 0},
        {AACENC_PEAK_BITRATE, peak_bitrate}, {AACENC_AFTERBURNER, 0},
        {AACENC_TRANSMUX, TT_MP4_RAW}
    };
    for (size_t i = 0; i < sizeof(parameters) / sizeof(parameters[0]); ++i) {
        if (aacEncoder_SetParam(encoder, parameters[i].parameter, parameters[i].value) != AACENC_OK) {
            fprintf(stderr, "Unsupported AAC-ELD parameter %x.\n", parameters[i].parameter);
            goto cleanup;
        }
    }
    if (aacEncEncode(encoder, NULL, NULL, NULL, NULL) != AACENC_OK ||
        aacEncInfo(encoder, &info) != AACENC_OK || !valid_configuration(&info) ||
        aacEncoder_GetParam(encoder, AACENC_BITRATEMODE) != (variable ? 5u : 0u) ||
        aacEncoder_GetParam(encoder, AACENC_PEAK_BITRATE) != peak_bitrate ||
        (!variable && aacEncoder_GetParam(encoder, AACENC_BITRATE) != bitrate)) {
        fprintf(stderr, "Cannot initialize the 480-sample AAC-ELD configuration.\n");
        goto cleanup;
    }
    if (!write_bytes("NPELD001", 8) || !write_u16(info.frameLength) ||
        !write_u16(info.confSize) || !write_bytes(info.confBuf, info.confSize) || fflush(stdout)) {
        fprintf(stderr, "Cannot write encoder configuration.\n");
        goto cleanup;
    }
    // FDK's CBR frames can exceed the rounded 10 ms byte budget by one byte.
    const unsigned max_au = (peak_bitrate + 799) / 800 + 1;
    audio_task = AvSetMmThreadCharacteristicsW(L"Audio", &task_index);
    for (;;) {
        size_t count = fread(pcm, 1, sizeof(pcm), stdin);
        if (ferror(stdin) || (count && count != sizeof(pcm))) {
            fprintf(stderr, "PCM input must contain complete 480-frame stereo blocks.\n");
            goto cleanup;
        }
        if (!count) break;
        if (encode(encoder, pcm, 0, max_au) < 0) goto cleanup;
    }
    for (int i = 0; i < 32; ++i) {
        int result = encode(encoder, pcm, 1, max_au);
        if (result < 0) goto cleanup;
        if (!result) { exit_code = 0; goto cleanup; }
    }
    fprintf(stderr, "AAC-ELD flush exceeded its limit.\n");
cleanup:
    if (audio_task) AvRevertMmThreadCharacteristics(audio_task);
    aacEncClose(&encoder);
    return exit_code;
}
