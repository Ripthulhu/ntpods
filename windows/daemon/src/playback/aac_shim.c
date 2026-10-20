#include <libavcodec/avcodec.h>
#include <libavutil/channel_layout.h>
#include <libavutil/opt.h>
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define AAC_SAMPLES 1024
#define AAC_PCM_BYTES (AAC_SAMPLES * 4)
#define AAC_F32_PCM_BYTES (AAC_SAMPLES * 8)
#define AAC_MAX_AU 1357
#define AAC_MAX_CONFIG 64

typedef struct {
    AVCodecContext *ctx;
    AVFrame *frame;
    AVPacket *packet;
    int64_t samples;
    int max_au;
} AacEnc;

void ntpods_aac_close(void *handle) {
    AacEnc *encoder = handle;
    if (!encoder) return;
    av_packet_free(&encoder->packet);
    av_frame_free(&encoder->frame);
    avcodec_free_context(&encoder->ctx);
    free(encoder);
}

void *ntpods_aac_open_at(uint32_t bitrate) {
    if (bitrate < 96000 || bitrate > 512000 || bitrate % 1000) return NULL;
    const AVCodec *codec = avcodec_find_encoder_by_name("aac");
    if (!codec) return NULL;
    AacEnc *encoder = calloc(1, sizeof(*encoder));
    if (!encoder) return NULL;
    encoder->ctx = avcodec_alloc_context3(codec);
    encoder->frame = av_frame_alloc();
    encoder->packet = av_packet_alloc();
    if (!encoder->ctx || !encoder->frame || !encoder->packet) goto fail;
    AVCodecContext *ctx = encoder->ctx;
    ctx->sample_rate = 48000;
    ctx->sample_fmt = AV_SAMPLE_FMT_FLTP;
    ctx->time_base = (AVRational){1, 48000};
    av_channel_layout_default(&ctx->ch_layout, 2);
    // Reserve END/alignment and LATM framing inside the negotiated ceiling.
    ctx->bit_rate = bitrate - 3400;
    encoder->max_au = (int)((uint64_t)bitrate * AAC_SAMPLES / (48000 * 8)) - 8;
    ctx->bit_rate_tolerance = 0;
    ctx->profile = AV_PROFILE_MPEG2_AAC_LOW;
    ctx->flags |= AV_CODEC_FLAG_GLOBAL_HEADER;
    // The two-loop coder can stall on transients under a strict frame budget.
    if (av_opt_set(ctx->priv_data, "aac_coder", "fast", 0) < 0 ||
        av_opt_set_int(ctx->priv_data, "aac_pns", 0, 0) < 0 ||
        av_opt_set_int(ctx->priv_data, "aac_ltp", 0, 0) < 0 ||
        av_opt_set_int(ctx->priv_data, "aac_pred", 0, 0) < 0 ||
        avcodec_open2(ctx, codec, NULL) < 0) goto fail;
    if (ctx->frame_size != AAC_SAMPLES || ctx->sample_rate != 48000 ||
        ctx->ch_layout.nb_channels != 2 || ctx->sample_fmt != AV_SAMPLE_FMT_FLTP ||
        ctx->profile != AV_PROFILE_AAC_LOW || ctx->extradata_size < 2 ||
        ctx->extradata_size > AAC_MAX_CONFIG || !ctx->extradata ||
        ctx->extradata[0] != 0x11 || ctx->extradata[1] != 0x90) goto fail;
    encoder->frame->nb_samples = AAC_SAMPLES;
    encoder->frame->format = ctx->sample_fmt;
    encoder->frame->sample_rate = ctx->sample_rate;
    if (av_channel_layout_copy(&encoder->frame->ch_layout, &ctx->ch_layout) < 0 ||
        av_frame_get_buffer(encoder->frame, 0) < 0) goto fail;
    return encoder;
fail:
    ntpods_aac_close(encoder);
    return NULL;
}

void *ntpods_aac_open(void) { return ntpods_aac_open_at(256000); }

int ntpods_aac_config(void *handle, uint8_t *out, int capacity) {
    AacEnc *encoder = handle;
    if (!encoder || !out || capacity < encoder->ctx->extradata_size) return -1;
    memcpy(out, encoder->ctx->extradata, encoder->ctx->extradata_size);
    return encoder->ctx->extradata_size;
}

static int send_frame(AacEnc *encoder) {
    encoder->frame->pts = encoder->samples;
    int result = avcodec_send_frame(encoder->ctx, encoder->frame);
    if (result == AVERROR(EAGAIN)) return 1;
    if (result < 0) return -1;
    encoder->samples += AAC_SAMPLES;
    return 0;
}

// Return 1 when the caller must read queued output before retrying this input.
int ntpods_aac_write(void *handle, const uint8_t *pcm, int length) {
    AacEnc *encoder = handle;
    if (!encoder || !pcm || length != AAC_PCM_BYTES ||
        encoder->samples > INT64_MAX - AAC_SAMPLES) return -1;
    if (av_frame_make_writable(encoder->frame) < 0) return -1;
    for (int channel = 0; channel < 2; channel++) {
        float *out = (float *)encoder->frame->data[channel];
        for (int sample = 0; sample < AAC_SAMPLES; sample++) {
            int offset = sample * 4 + channel * 2;
            int value = pcm[offset] | (pcm[offset + 1] << 8);
            if (value >= 32768) value -= 65536;
            out[sample] = (float)value / 32768.0f;
        }
    }
    return send_frame(encoder);
}

int ntpods_aac_write_f32(void *handle, const uint8_t *pcm, int length) {
    AacEnc *encoder = handle;
    if (!encoder || !pcm || length != AAC_F32_PCM_BYTES || sizeof(float) != 4 ||
        encoder->samples > INT64_MAX - AAC_SAMPLES) return -1;
    float input[AAC_SAMPLES * 2];
    for (int sample = 0; sample < AAC_SAMPLES * 2; ++sample) {
        const uint8_t *bytes = pcm + sample * 4;
        const uint32_t bits = bytes[0] | ((uint32_t)bytes[1] << 8) |
            ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
        memcpy(&input[sample], &bits, sizeof(bits));
        if (!isfinite(input[sample])) return -1;
        if (input[sample] < -1.0f) input[sample] = -1.0f;
        if (input[sample] > 1.0f) input[sample] = 1.0f;
    }
    if (av_frame_make_writable(encoder->frame) < 0) return -1;
    for (int channel = 0; channel < 2; ++channel) {
        float *out = (float *)encoder->frame->data[channel];
        for (int sample = 0; sample < AAC_SAMPLES; ++sample)
            out[sample] = input[sample * 2 + channel];
    }
    return send_frame(encoder);
}

int ntpods_aac_read(void *handle, uint8_t *out, int capacity) {
    AacEnc *encoder = handle;
    if (!encoder || !out || capacity < AAC_MAX_AU) return -1;
    av_packet_unref(encoder->packet);
    int result = avcodec_receive_packet(encoder->ctx, encoder->packet);
    if (result == AVERROR(EAGAIN) || result == AVERROR_EOF) return 0;
    if (result < 0) return -1;
    int size = encoder->packet->size;
    if (size < 1 || size > encoder->max_au || !encoder->packet->data) {
        av_packet_unref(encoder->packet);
        return -1;
    }
    memcpy(out, encoder->packet->data, size);
    av_packet_unref(encoder->packet);
    return size;
}
