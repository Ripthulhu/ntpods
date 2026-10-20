#include <assert.h>
#include <float.h>
#include <math.h>
#include <stdio.h>
#include <time.h>
#define main aac_worker_main
#include "../src/playback/aac_worker.c"
#undef main

#define PI 3.14159265358979323846
#define TONE_BLOCKS 144

static void put_float_le(uint8_t *bytes, float value) {
    uint32_t bits;
    assert(sizeof(value) == sizeof(bits));
    memcpy(&bits, &value, sizeof(bits));
    for (int byte = 0; byte < 4; ++byte) bytes[byte] = (uint8_t)(bits >> (byte * 8));
}

static AacEnc *fixture(uint32_t bitrate) {
    AacEnc *encoder = ntpods_aac_open_at(bitrate);
    assert(encoder);
    int64_t value = -1;
    assert(av_opt_get_int(encoder->ctx->priv_data, "aac_coder", 0, &value) == 0);
    assert(value == 2);
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
    AacEnc *encoder = fixture(256000);
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

static void float_validation_and_backpressure(void) {
    AacEnc *encoder = fixture(256000);
    uint8_t pcm[AAC_F32_PCM_BYTES + 1], output[AAC_MAX_AU];
    for (int sample = 0; sample < AAC_SAMPLES * 2; ++sample)
        put_float_le(pcm + sample * 4, 0.5f);
    for (int channel = 0; channel < 2; ++channel)
        for (int sample = 0; sample < AAC_SAMPLES; ++sample)
            ((float *)encoder->frame->data[channel])[sample] = 0.25f;
    assert(ntpods_aac_write_f32(NULL, pcm, AAC_F32_PCM_BYTES) == -1);
    assert(ntpods_aac_write_f32(encoder, NULL, AAC_F32_PCM_BYTES) == -1);
    const int bad_lengths[] = {-1, 0, AAC_F32_PCM_BYTES - 1, AAC_F32_PCM_BYTES + 1};
    for (int i = 0; i < sizeof(bad_lengths) / sizeof(bad_lengths[0]); ++i)
        assert(ntpods_aac_write_f32(encoder, pcm, bad_lengths[i]) == -1);
    const float invalid[] = {NAN, INFINITY, -INFINITY};
    const int positions[] = {0, AAC_SAMPLES, AAC_SAMPLES * 2 - 1};
    for (int value = 0; value < sizeof(invalid) / sizeof(invalid[0]); ++value) {
        for (int position = 0; position < sizeof(positions) / sizeof(positions[0]); ++position) {
            uint8_t *bytes = pcm + positions[position] * 4;
            put_float_le(bytes, invalid[value]);
            assert(ntpods_aac_write_f32(encoder, pcm, AAC_F32_PCM_BYTES) == -1);
            assert(encoder->samples == 0);
            for (int channel = 0; channel < 2; ++channel)
                for (int sample = 0; sample < AAC_SAMPLES; ++sample)
                    assert(((float *)encoder->frame->data[channel])[sample] == 0.25f);
            put_float_le(bytes, 0.5f);
        }
    }
    assert(encoder->samples == 0);
    encoder->samples = INT64_MAX - AAC_SAMPLES + 1;
    assert(ntpods_aac_write_f32(encoder, pcm, AAC_F32_PCM_BYTES) == -1);
    assert(encoder->samples == INT64_MAX - AAC_SAMPLES + 1);
    encoder->samples = 0;
    const float finite[] = {-FLT_MAX, -2.0f, -1.0f, -0.5f, 0.0f, 0.5f, 1.0f, 2.0f, FLT_MAX};
    const float clamped[] = {-1.0f, -1.0f, -1.0f, -0.5f, 0.0f, 0.5f, 1.0f, 1.0f, 1.0f};
    for (int sample = 0; sample < AAC_SAMPLES * 2; ++sample)
        put_float_le(pcm + sample * 4, finite[sample % 9]);
    assert(ntpods_aac_write_f32(encoder, pcm, AAC_F32_PCM_BYTES) == 0);
    assert(encoder->samples == AAC_SAMPLES);
    for (int channel = 0; channel < 2; ++channel)
        for (int sample = 0; sample < AAC_SAMPLES; ++sample)
            assert(((float *)encoder->frame->data[channel])[sample] == clamped[(sample * 2 + channel) % 9]);
    int blocked = 0;
    for (int i = 0; i < 8; ++i) {
        int64_t before = encoder->samples;
        int result = ntpods_aac_write_f32(encoder, pcm, AAC_F32_PCM_BYTES);
        assert(result >= 0);
        if (result == 1) {
            assert(encoder->samples == before);
            int size, drained = 0;
            while ((size = ntpods_aac_read(encoder, output, sizeof(output))) > 0) ++drained;
            assert(size == 0 && drained > 0);
            assert(ntpods_aac_write_f32(encoder, pcm, AAC_F32_PCM_BYTES) == 0);
            assert(encoder->samples == before + AAC_SAMPLES);
            blocked = 1;
            break;
        }
    }
    assert(blocked);
    ntpods_aac_close(encoder);
}

static int compare_encoded(AacEnc *left, AacEnc *right) {
    uint8_t left_bytes[AAC_MAX_AU], right_bytes[AAC_MAX_AU];
    int left_size, right_size, packets = 0;
    do {
        left_size = ntpods_aac_read(left, left_bytes, sizeof(left_bytes));
        right_size = ntpods_aac_read(right, right_bytes, sizeof(right_bytes));
        assert(left_size >= 0 && left_size == right_size);
        assert(!memcmp(left_bytes, right_bytes, (size_t)left_size));
        packets += left_size > 0;
    } while (left_size > 0);
    return packets;
}

static void float_s16_lattice_matches(void) {
    AacEnc *narrow = fixture(256000), *wide = fixture(256000);
    uint32_t random = 0x2a3849;
    int packets = 0;
    for (int block = 0; block < 32; ++block) {
        uint8_t pcm[AAC_PCM_BYTES], float_pcm[AAC_F32_PCM_BYTES];
        for (int sample = 0; sample < AAC_SAMPLES * 2; ++sample) {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            const uint16_t bits = (uint16_t)random;
            const int value = bits >= 32768 ? (int)bits - 65536 : bits;
            pcm[sample * 2] = (uint8_t)bits;
            pcm[sample * 2 + 1] = (uint8_t)(bits >> 8);
            put_float_le(float_pcm + sample * 4, (float)value / 32768.0f);
        }
        assert(ntpods_aac_write(narrow, pcm, sizeof(pcm)) == 0);
        assert(ntpods_aac_write_f32(wide, float_pcm, sizeof(float_pcm)) == 0);
        assert(narrow->samples == wide->samples);
        packets += compare_encoded(narrow, wide);
    }
    assert(avcodec_send_frame(narrow->ctx, NULL) == 0);
    assert(avcodec_send_frame(wide->ctx, NULL) == 0);
    packets += compare_encoded(narrow, wide);
    assert(packets == 33);
    ntpods_aac_close(narrow);
    ntpods_aac_close(wide);
}

typedef struct {
    AVCodecContext *ctx;
    AVPacket *packet;
    AVFrame *frame;
    double real[2][2];
    double imag[2][2];
    double energy[2];
    double pulse_on_energy[2], pulse_off_energy[2];
    int pulse_metrics, initial_padding, pulse_on_samples, pulse_off_samples;
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
                decoder->energy[channel] += (double)value * value;
                if (decoder->pulse_metrics) {
                    int block = (index - decoder->initial_padding) / AAC_SAMPLES % 24;
                    if (block >= 3 && block < 9) decoder->pulse_on_energy[channel] += (double)value * value;
                    if (block >= 15 && block < 21) decoder->pulse_off_energy[channel] += (double)value * value;
                    if (!channel) {
                        decoder->pulse_on_samples += block >= 3 && block < 9;
                        decoder->pulse_off_samples += block >= 15 && block < 21;
                    }
                }
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

static void stereo_roundtrip(uint32_t bitrate) {
    AacEnc *encoder = fixture(bitrate);
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

static void quiet_float_roundtrip(int pulsed, int s16_silence) {
    AacEnc *encoder = fixture(256000);
    const AVCodec *codec = avcodec_find_decoder_by_name("aac");
    Decoder decoder = {0};
    decoder.ctx = avcodec_alloc_context3(codec);
    decoder.packet = av_packet_alloc();
    decoder.frame = av_frame_alloc();
    decoder.pulse_metrics = pulsed;
    decoder.initial_padding = encoder->ctx->initial_padding;
    assert(decoder.ctx && decoder.packet && decoder.frame);
    assert(decoder.initial_padding >= 0 && decoder.initial_padding <= 4 * AAC_SAMPLES);
    decoder.ctx->extradata = av_mallocz(AAC_MAX_CONFIG + AV_INPUT_BUFFER_PADDING_SIZE);
    assert(decoder.ctx->extradata);
    decoder.ctx->extradata_size = ntpods_aac_config(encoder, decoder.ctx->extradata, AAC_MAX_CONFIG);
    assert(avcodec_open2(decoder.ctx, codec, NULL) == 0);
    int packets = 0;
    uint8_t output[AAC_MAX_AU];
    for (int block = 0; block < TONE_BLOCKS; ++block) {
        uint8_t pcm[AAC_F32_PCM_BYTES], s16[AAC_PCM_BYTES] = {0};
        for (int sample = 0; sample < AAC_SAMPLES; ++sample) {
            for (int channel = 0; channel < 2; ++channel) {
                double phase = 2 * PI * (block * AAC_SAMPLES + sample) * (channel ? 880 : 440) / 48000;
                float value = pulsed && block % 24 >= 12 ? 0.0f : (float)(sin(phase) * 1e-5);
                assert((int16_t)(value * 32768.0f) == 0);
                put_float_le(pcm + (sample * 2 + channel) * 4, value);
            }
        }
        assert((s16_silence ? ntpods_aac_write(encoder, s16, sizeof(s16)) :
                ntpods_aac_write_f32(encoder, pcm, sizeof(pcm))) == 0);
        int size;
        while ((size = ntpods_aac_read(encoder, output, sizeof(output))) > 0) {
            assert(size <= encoder->max_au);
            decode(&decoder, output, size);
            ++packets;
        }
        assert(size == 0);
    }
    assert(avcodec_send_frame(encoder->ctx, NULL) == 0);
    int size;
    while ((size = ntpods_aac_read(encoder, output, sizeof(output))) > 0) {
        decode(&decoder, output, size);
        ++packets;
    }
    assert(size == 0 && packets == TONE_BLOCKS + 1 && decoder.measured > 0);
    assert(decoder.samples == (TONE_BLOCKS + 1) * AAC_SAMPLES);
    for (int channel = 0; channel < 2; ++channel) {
        const double rms = sqrt(decoder.energy[channel] / decoder.measured);
        const double wanted = hypot(decoder.real[channel][channel], decoder.imag[channel][channel]);
        const double other = hypot(decoder.real[channel][1 - channel], decoder.imag[channel][1 - channel]);
        const double amplitude = 2 * wanted / decoder.measured;
        if (s16_silence) assert(rms < 1e-9);
        else {
            assert(amplitude > (pulsed ? 1e-6 : 3e-6) && amplitude < 2e-5);
            assert(rms > 2e-6 && rms < 2e-5 && other / wanted < 0.05);
            if (pulsed) {
                assert(decoder.pulse_on_samples > 0 && decoder.pulse_off_samples > 0);
                const double on = decoder.pulse_on_energy[channel] / decoder.pulse_on_samples;
                const double off = decoder.pulse_off_energy[channel] / decoder.pulse_off_samples;
                assert(on > 1e-11 && on < 2e-10 && off < on * 0.01);
            }
        }
        printf("AAC quiet %s channel %d: %.9g amplitude, %.9g RMS\n",
               s16_silence ? "S16 silence" : pulsed ? "pulsed F32" : "continuous F32", channel, amplitude, rms);
    }
    av_frame_free(&decoder.frame);
    av_packet_free(&decoder.packet);
    avcodec_free_context(&decoder.ctx);
    ntpods_aac_close(encoder);
}

static void peak_and_speed(uint32_t bitrate, int verbose) {
    AacEnc *encoder = fixture(bitrate);
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
    assert(packets >= 498 && peak <= encoder->max_au);
    if (verbose) printf("AAC %u kbps noise/transients: %d AUs, peak %d/%d bytes, %.3fs for 10.667s audio\n", bitrate/1000, packets, peak, encoder->max_au, seconds);
    assert(seconds < 10.667);
    ntpods_aac_close(encoder);
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    av_log_set_level(AV_LOG_ERROR);
    invalid_buffers_and_backpressure();
    float_validation_and_backpressure();
    float_s16_lattice_matches();
    quiet_float_roundtrip(0, 1);
    quiet_float_roundtrip(0, 0);
    quiet_float_roundtrip(1, 0);
    uint32_t parsed;
    int pcm_f32;
    char *defaults[] = {"worker"};
    assert(parse_options(1, defaults, &parsed, &pcm_f32) && parsed == 256000 && !pcm_f32);
    const char *invalid[] = {"", "-96000", "+96000", "96000x", "9e4", "95000", "96001", "513000", "4294967296"};
    for (int i = 0; i < sizeof(invalid) / sizeof(invalid[0]); i++) {
        char *args[] = {"worker", "--bitrate", (char *)invalid[i]};
        assert(!parse_options(3, args, &parsed, &pcm_f32));
    }
    char *wrong_flag[] = {"worker", "--quality", "256000"};
    assert(!parse_options(3, wrong_flag, &parsed, &pcm_f32));
    char *missing_bitrate[] = {"worker", "--bitrate"};
    assert(!parse_options(2, missing_bitrate, &parsed, &pcm_f32));
    char *float_default[] = {"worker", "--pcm-f32"};
    char *float_rate[] = {"worker", "--pcm-f32", "--bitrate", "128000"};
    char *rate_float[] = {"worker", "--bitrate", "128000", "--pcm-f32"};
    char *duplicate_float[] = {"worker", "--pcm-f32", "--pcm-f32"};
    char *duplicate_rate[] = {"worker", "--bitrate", "128000", "--bitrate", "128000"};
    assert(parse_options(2, float_default, &parsed, &pcm_f32) && parsed == 256000 && pcm_f32);
    assert(parse_options(4, float_rate, &parsed, &pcm_f32) && parsed == 128000 && pcm_f32);
    assert(parse_options(4, rate_float, &parsed, &pcm_f32) && parsed == 128000 && pcm_f32);
    assert(!parse_options(3, duplicate_float, &parsed, &pcm_f32));
    assert(!parse_options(5, duplicate_rate, &parsed, &pcm_f32));
    const uint32_t rates[] = {96000, 97000, 128000, 256000, 320000, 511000, 512000};
    assert(!ntpods_aac_open_at(95000) && !ntpods_aac_open_at(513000) && !ntpods_aac_open_at(128001));
    for (int i = 0; i < sizeof(rates) / sizeof(rates[0]); i++) {
        printf("Testing AAC %u kbps\n", rates[i]/1000);
        stereo_roundtrip(rates[i]);
        peak_and_speed(rates[i], 1);
    }
    puts("Checking noise/transients at all 417 AAC rates");
    for (uint32_t rate = 96000; rate <= 512000; rate += 1000) {
        char number[7];
        snprintf(number, sizeof(number), "%u", rate);
        char *args[] = {"worker", "--bitrate", number};
        assert(parse_options(3, args, &parsed, &pcm_f32) && parsed == rate && !pcm_f32);
        peak_and_speed(rate, 0);
    }
    puts("AAC encoder: S16/F32 bounds, backpressure, quiet tones, LC tools, stereo pitch and peak bitrate passed");
    return 0;
}
