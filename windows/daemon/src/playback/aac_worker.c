#include <fcntl.h>
#include <io.h>
#include <stdio.h>
#include <windows.h>
#include <avrt.h>
#include "aac_shim.c"

static int parse_options(int argc, char **argv, uint32_t *bitrate, int *pcm_f32) {
    *bitrate = 256000;
    *pcm_f32 = 0;
    int seen_bitrate = 0;
    for (int index = 1; index < argc; ++index) {
        if (!strcmp(argv[index], "--pcm-f32") && !*pcm_f32) {
            *pcm_f32 = 1;
        } else if (!strcmp(argv[index], "--bitrate") && !seen_bitrate && ++index < argc) {
            const size_t length = strlen(argv[index]);
            if (length < 5 || length > 6 || strspn(argv[index], "0123456789") != length) return 0;
            *bitrate = (uint32_t)strtoul(argv[index], NULL, 10);
            seen_bitrate = 1;
        } else return 0;
    }
    return *bitrate >= 96000 && *bitrate <= 512000 && *bitrate % 1000 == 0;
}

static int write_bytes(const void *data, size_t size) {
    return fwrite(data, 1, size, stdout) == size;
}

static int write_u16(unsigned value) {
    const uint8_t bytes[2] = {(uint8_t)value, (uint8_t)(value >> 8)};
    return write_bytes(bytes, sizeof(bytes));
}

static int drain(AacEnc *encoder) {
    uint8_t output[AAC_MAX_AU];
    int size;
    while ((size = ntpods_aac_read(encoder, output, sizeof(output))) > 0) {
        if (!write_u16((unsigned)size) || !write_bytes(output, (size_t)size)) return 0;
    }
    return size == 0 && fflush(stdout) == 0;
}

int main(int argc, char **argv) {
    uint32_t bitrate;
    int pcm_f32;
    if (!parse_options(argc, argv, &bitrate, &pcm_f32)) {
        fprintf(stderr, "Use --bitrate with 96000..512000 bits per second in 1000-bit steps, optionally --pcm-f32.\n");
        return 1;
    }
    if (_setmode(_fileno(stdin), _O_BINARY) == -1 ||
        _setmode(_fileno(stdout), _O_BINARY) == -1) return 1;
    if (pcm_f32) {
        const uint8_t expected[16] = {'N','P','F','3','2','0','0','1',0x80,0xbb,0,0,2,0,0,4};
        uint8_t preamble[sizeof(expected)];
        if (fread(preamble, 1, sizeof(preamble), stdin) != sizeof(preamble) ||
            memcmp(preamble, expected, sizeof(expected))) return 1;
    }
    av_log_set_level(AV_LOG_ERROR);
    AacEnc *encoder = ntpods_aac_open_at(bitrate);
    if (!encoder) return 1;
    int exit_code = 1;
    DWORD task_index = 0;
    HANDLE audio_task = AvSetMmThreadCharacteristicsW(L"Audio", &task_index);
    uint8_t config[AAC_MAX_CONFIG], pcm[AAC_F32_PCM_BYTES];
    const int pcm_bytes = pcm_f32 ? AAC_F32_PCM_BYTES : AAC_PCM_BYTES;
    int size = ntpods_aac_config(encoder, config, sizeof(config));
    if (size < 2 || !write_bytes("NPAAC001", 8) || !write_u16(AAC_SAMPLES) ||
        !write_u16((unsigned)size) || !write_bytes(config, (size_t)size) || fflush(stdout)) goto done;
    for (;;) {
        size_t bytes = fread(pcm, 1, (size_t)pcm_bytes, stdin);
        if (ferror(stdin)) goto done;
        if (!bytes && feof(stdin)) break;
        if (bytes != (size_t)pcm_bytes) goto done;
        int status = pcm_f32 ? ntpods_aac_write_f32(encoder, pcm, pcm_bytes)
            : ntpods_aac_write(encoder, pcm, pcm_bytes);
        if (status == 1 && drain(encoder))
            status = pcm_f32 ? ntpods_aac_write_f32(encoder, pcm, pcm_bytes)
                : ntpods_aac_write(encoder, pcm, pcm_bytes);
        if (status || !drain(encoder)) goto done;
    }
    if (avcodec_send_frame(encoder->ctx, NULL) == 0 && drain(encoder)) exit_code = 0;
done:
    if (audio_task) AvRevertMmThreadCharacteristics(audio_task);
    ntpods_aac_close(encoder);
    return exit_code;
}
