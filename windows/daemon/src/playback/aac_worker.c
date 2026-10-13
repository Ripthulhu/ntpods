#include <fcntl.h>
#include <io.h>
#include <stdio.h>
#include <windows.h>
#include <avrt.h>
#include "aac_shim.c"

static int parse_bitrate(int argc, char **argv, uint32_t *bitrate) {
    *bitrate = 256000;
    if (argc == 1) return 1;
    if (argc != 3 || strcmp(argv[1], "--bitrate")) return 0;
    size_t length = strlen(argv[2]);
    if (length < 5 || length > 6 || strspn(argv[2], "0123456789") != length) return 0;
    *bitrate = (uint32_t)strtoul(argv[2], NULL, 10);
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
    if (!parse_bitrate(argc, argv, &bitrate)) {
        fprintf(stderr, "Use --bitrate with 96000..512000 bits per second in 1000-bit steps.\n");
        return 1;
    }
    if (_setmode(_fileno(stdin), _O_BINARY) == -1 ||
        _setmode(_fileno(stdout), _O_BINARY) == -1) return 1;
    av_log_set_level(AV_LOG_ERROR);
    AacEnc *encoder = ntpods_aac_open_at(bitrate);
    if (!encoder) return 1;
    int exit_code = 1;
    DWORD task_index = 0;
    HANDLE audio_task = AvSetMmThreadCharacteristicsW(L"Audio", &task_index);
    uint8_t config[AAC_MAX_CONFIG], pcm[AAC_PCM_BYTES];
    int size = ntpods_aac_config(encoder, config, sizeof(config));
    if (size < 2 || !write_bytes("NPAAC001", 8) || !write_u16(AAC_SAMPLES) ||
        !write_u16((unsigned)size) || !write_bytes(config, (size_t)size) || fflush(stdout)) goto done;
    for (;;) {
        size_t bytes = fread(pcm, 1, sizeof(pcm), stdin);
        if (!bytes && feof(stdin)) break;
        if (bytes != sizeof(pcm) || ferror(stdin) ||
            ntpods_aac_write(encoder, pcm, sizeof(pcm)) != 0 || !drain(encoder)) goto done;
    }
    if (avcodec_send_frame(encoder->ctx, NULL) == 0 && drain(encoder)) exit_code = 0;
done:
    if (audio_task) AvRevertMmThreadCharacteristics(audio_task);
    ntpods_aac_close(encoder);
    return exit_code;
}
