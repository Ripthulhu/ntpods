#include <assert.h>
#include <math.h>
#include <stdio.h>
#include <time.h>
#include "../src/playback/aac_shim.c"

#define PI 3.14159265358979323846
#define TONE_BLOCKS 144

static AacEnc *fixture(void) {
    AacEnc *encoder = ntpods_aac_open();
    assert(encoder);
    int64_t value = -1;
    for (int i = 0; i < 3; i++) {
        const char *name[] = {"aac_pns", "aac_ltp", "aac_pred"};
        assert(av_opt_get_int(encoder->ctx->priv_data, name[i], 0, &value) == 0);
        assert(value == 0);
    }
    uint8_t asc[AAC_MAX_CONFIG];
    int size = ntpods_aac_config(encoder, asc, sizeof(asc));
    const uint8_t expected[] = {0x11, 0x90, 0x56, 0xe5, 0};
    assert(size == sizeof(expected) && memcmp(asc, expected, size) == 0);
    return encoder;
}

static void invalid_buffers_and_backpressure(void) {
    AacEnc *encoder = fixture();
    uint8_t pcm[AAC_PCM_BYTES + 1] = {0};
    uint8_t output[AAC_MAX_AU];
    memset(output, 0x5a, sizeof(output));
    assert(ntpods_aac_write(NULL, pcm, AAC_PCM_BYTES) == -1);
    assert(ntpods_aac_write(encoder, NULL, AAC_PCM_BYTES) == -1);
    assert(ntpods_aac_write(encoder, pcm, AAC_PCM_BYTES - 1) == -1);
    assert(ntpods_aac_write(encoder, pcm, AAC_PCM_BYTES + 1) == -1);
    assert(ntpods_aac_config(encoder, output, 1) == -1);
    assert(ntpods_aac_read(encoder, output, AAC_MAX_AU - 1) == -1);
    for (int i = 0; i < sizeof(output); i++) assert(output[i] == 0x5a);
    int blocked = 0;
    for (int i = 0; i < 8; i++) {
        int64_t before = encoder->samples;
        int result = ntpods_aac_write(encoder, pcm, AAC_PCM_BYTES);
        assert(result >= 0);
        if (result == 1) {
            assert(encoder->samples == before);
            blocked = 1;
            break;
        }
    }
    assert(blocked);
    assert(ntpods_aac_read(encoder, output, sizeof(output)) > 0);
    ntpods_aac_close(encoder);
    ntpods_aac_close(NULL);
}

typedef struct {
    AVCodecContext *ctx;
    AVPacket *packet;
    AVFrame *frame;
    double real[2][2];
    double imag[2][2];
    int samples;
    int measured;
} Decoder;

static void decode(Decoder *decoder, const uint8_t *data, int length) {
    assert(av_new_packet(decoder->packet, length) == 0);
    memcpy(decoder->packet->data, data, length);
    assert(avcodec_send_packet(decoder->ctx, decoder->packet) == 0);
    av_packet_unref(decoder->packet);
    int result;
    while ((result = avcodec_receive_frame(decoder->ctx, decoder->frame)) == 0) {
        AVFrame *frame = decoder->frame;
        assert(frame->sample_rate == 48000 && frame->ch_layout.nb_channels == 2);
        assert(frame->nb_samples == AAC_SAMPLES && frame->format == AV_SAMPLE_FMT_FLTP);
        assert(!frame->decode_error_flags && !(frame->flags & AV_FRAME_FLAG_CORRUPT));
        for (int sample = 0; sample < frame->nb_samples; sample++) {
            int index = decoder->samples++;
            for (int channel = 0; channel < 2; channel++) {
                float value = ((float *)frame->data[channel])[sample];
                assert(isfinite(value) && fabsf(value) < 1.0f);
                if (index < 4096 || index >= TONE_BLOCKS * AAC_SAMPLES - 4096) continue;
                for (int tone = 0; tone < 2; tone++) {
                    double phase = 2 * PI * index * (tone == 0 ? 440 : 880) / 48000;
                    decoder->real[channel][tone] += value * cos(phase);
                    decoder->imag[channel][tone] += value * sin(phase);
                }
            }
            if (index >= 4096 && index < TONE_BLOCKS * AAC_SAMPLES - 4096) decoder->measured++;
        }
        av_frame_unref(frame);
    }
    assert(result == AVERROR(EAGAIN));
}

static void stereo_roundtrip(void) {
    AacEnc *encoder = fixture();
    const AVCodec *codec = avcodec_find_decoder_by_name("aac");
    Decoder decoder = {0};
    decoder.ctx = avcodec_alloc_context3(codec);
    decoder.packet = av_packet_alloc();
    decoder.frame = av_frame_alloc();
    assert(decoder.ctx && decoder.packet && decoder.frame);
    decoder.ctx->extradata = av_mallocz(AAC_MAX_CONFIG + AV_INPUT_BUFFER_PADDING_SIZE);
    assert(decoder.ctx->extradata);
    decoder.ctx->extradata_size = ntpods_aac_config(encoder, decoder.ctx->extradata, AAC_MAX_CONFIG);
    assert(avcodec_open2(decoder.ctx, codec, NULL) == 0);
    uint8_t output[AAC_MAX_AU];
    int packets = 0;
    for (int block = 0; block < TONE_BLOCKS; block++) {
        uint8_t pcm[AAC_PCM_BYTES];
        for (int sample = 0; sample < AAC_SAMPLES; sample++) {
            for (int channel = 0; channel < 2; channel++) {
                double phase = 2 * PI * (block * AAC_SAMPLES + sample) * (channel == 0 ? 440 : 880) / 48000;
                int16_t value = (int16_t)(sin(phase) * 8000);
                pcm[sample * 4 + channel * 2] = (uint8_t)value;
                pcm[sample * 4 + channel * 2 + 1] = (uint8_t)((uint16_t)value >> 8);
            }
        }
        assert(ntpods_aac_write(encoder, pcm, sizeof(pcm)) == 0);
        int size;
        while ((size = ntpods_aac_read(encoder, output, sizeof(output))) > 0) {
            decode(&decoder, output, size);
            packets++;
        }
        assert(size == 0);
        if (block == 4) assert(packets >= 3);
    }
    assert(avcodec_send_frame(encoder->ctx, NULL) == 0);
    int size;
    while ((size = ntpods_aac_read(encoder, output, sizeof(output))) > 0) {
        decode(&decoder, output, size);
        packets++;
    }
    assert(size == 0 && packets == TONE_BLOCKS + 1);
    assert(decoder.samples == (TONE_BLOCKS + 1) * AAC_SAMPLES);
    for (int channel = 0; channel < 2; channel++) {
        double wanted = hypot(decoder.real[channel][channel], decoder.imag[channel][channel]);
        double other = hypot(decoder.real[channel][1 - channel], decoder.imag[channel][1 - channel]);
        double amplitude = 2 * wanted / decoder.measured;
        assert(amplitude > 0.22 && amplitude < 0.27);
        assert(other / wanted < 0.01);
        printf("AAC channel %d: %.4f amplitude, %.5f crosstalk\n", channel, amplitude, other / wanted);
    }
    av_frame_free(&decoder.frame);
    av_packet_free(&decoder.packet);
    avcodec_free_context(&decoder.ctx);
    ntpods_aac_close(encoder);
}

static void peak_and_speed(void) {
    AacEnc *encoder = fixture();
    uint32_t random = 0x194838;
    uint8_t pcm[AAC_PCM_BYTES], output[AAC_MAX_AU];
    int peak = 0, packets = 0;
    clock_t start = clock();
    for (int block = 0; block < 500; block++) {
        for (int sample = 0; sample < AAC_SAMPLES * 2; sample++) {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            uint16_t value = (uint16_t)random;
            if (block % 3 == 1) value = sample % 64 == 0 ? 32767 : 0;
            if (block % 3 == 2) value = sample % 2 == 0 ? 32767 : 32768;
            pcm[sample * 2] = (uint8_t)value;
            pcm[sample * 2 + 1] = (uint8_t)(value >> 8);
        }
        assert(ntpods_aac_write(encoder, pcm, sizeof(pcm)) == 0);
        int size;
        while ((size = ntpods_aac_read(encoder, output, sizeof(output))) > 0) {
            if (size > peak) peak = size;
            packets++;
        }
        assert(size == 0);
    }
    double seconds = (double)(clock() - start) / CLOCKS_PER_SEC;
    assert(packets >= 498 && peak <= AAC_MAX_AU);
    printf("AAC noise/transients: %d AUs, peak %d bytes, %.3fs for 10.667s audio\n", packets, peak, seconds);
    assert(seconds < 10.667);
    ntpods_aac_close(encoder);
}

int main(void) {
    av_log_set_level(AV_LOG_ERROR);
    invalid_buffers_and_backpressure();
    stereo_roundtrip();
    peak_and_speed();
    puts("AAC encoder: PCM bounds, backpressure, LC tools, stereo pitch and peak bitrate passed");
    return 0;
}
