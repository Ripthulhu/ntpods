// AAC-ELD decoding through FFmpeg libavcodec + libswresample (LGPL).
// The C ABI keeps FFmpeg structs out of Rust and returns 48 kHz mono s16 PCM.
#include <libavcodec/avcodec.h>
#include <libavutil/channel_layout.h>
#include <libavutil/frame.h>
#include <libavutil/mem.h>
#include <libswresample/swresample.h>
#include <math.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define OUT_RATE 48000
#define FRAME_SAMPLES 480
// Inherited source-clock assumption: 64 kHz (LibrePods PR #655's ELD_SAMPLE_RATE),
// although the four-byte ASC advertises 48 kHz. Upstream reported low pitch and
// excess queued samples without this conversion. Verify against actual 9A348
// recordings before accepting pitch and long-run drift; the ASC alone is not proof.
#define IN_RATE 64000
// Apply microphone gain with tanh soft limiting before resampling to s16.
#define GAIN 3.0f

typedef struct {
    AVCodecContext *ctx;
    AVPacket *pkt;
    AVFrame *frame;
    SwrContext *swr;
    int in_rate;  // decoder output rate, learned from the first frame
} EldDec;

void *eld_open(const uint8_t *asc, int asc_len, int sample_rate) {
    if (!asc || asc_len <= 0 || asc_len > 64 || sample_rate <= 0) return NULL;
    const AVCodec *c = avcodec_find_decoder_by_name("aac");
    if (!c) return NULL;
    AVCodecContext *ctx = avcodec_alloc_context3(c);
    if (!ctx) return NULL;
    ctx->extradata = (uint8_t *)av_malloc(asc_len + AV_INPUT_BUFFER_PADDING_SIZE);
    if (!ctx->extradata) { avcodec_free_context(&ctx); return NULL; }
    memset(ctx->extradata, 0, asc_len + AV_INPUT_BUFFER_PADDING_SIZE);
    memcpy(ctx->extradata, asc, asc_len);
    ctx->extradata_size = asc_len;
    ctx->sample_rate = sample_rate;  // channels/rate ultimately come from the ASC
    if (avcodec_open2(ctx, c, NULL) < 0) { avcodec_free_context(&ctx); return NULL; }
    EldDec *d = (EldDec *)calloc(1, sizeof(EldDec));
    if (!d) { avcodec_free_context(&ctx); return NULL; }
    d->ctx = ctx;
    d->pkt = av_packet_alloc();
    d->frame = av_frame_alloc();
    d->swr = NULL;
    d->in_rate = 0;
    if (!d->pkt || !d->frame) {
        av_frame_free(&d->frame);
        av_packet_free(&d->pkt);
        avcodec_free_context(&d->ctx);
        free(d);
        return NULL;
    }
    return d;
}

// The fixed ASC yields 480 mono float samples at a signalled 48 kHz. Reject
// corrupt output before gain can turn it into a full-scale transient.
static int eld_convert_frame(EldDec *d, int16_t *out, int out_cap) {
    AVFrame *frame = d->frame;
    if (!out || out_cap <= 0 || frame->ch_layout.nb_channels != 1 ||
        frame->nb_samples != FRAME_SAMPLES || frame->sample_rate != OUT_RATE ||
        (frame->format != AV_SAMPLE_FMT_FLTP && frame->format != AV_SAMPLE_FMT_FLT) ||
        frame->decode_error_flags ||
        (frame->flags & (AV_FRAME_FLAG_CORRUPT | AV_FRAME_FLAG_DISCARD)) ||
        !frame->data[0]) return -1;
    const float *samples = (const float *)frame->data[0];
    for (int i = 0; i < frame->nb_samples; i++) {
        if (!isfinite(samples[i])) return -1;
    }
    // Decoder references may share these buffers with prediction history.
    if (av_frame_make_writable(frame) < 0) return -1;
    if (!d->swr) {
        AVChannelLayout mono;
        av_channel_layout_default(&mono, 1);
        // Packed and planar float have the same layout for one channel.
        int result = swr_alloc_set_opts2(&d->swr, &mono, AV_SAMPLE_FMT_S16, OUT_RATE,
                            &mono, AV_SAMPLE_FMT_FLT, IN_RATE, 0, NULL);
        av_channel_layout_uninit(&mono);
        if (result < 0 || !d->swr || swr_init(d->swr) < 0) {
            swr_free(&d->swr);
            return -1;
        }
        d->in_rate = frame->sample_rate;
    }
    // Do not silently queue unbounded samples behind an undersized output.
    int required = swr_get_out_samples(d->swr, frame->nb_samples);
    if (required < 0 || required > out_cap) return -1;
    float *writable = (float *)frame->data[0];
    for (int i = 0; i < frame->nb_samples; i++) {
        writable[i] = tanhf(writable[i] * GAIN);
    }
    uint8_t *outp = (uint8_t *)out;
    return swr_convert(d->swr, &outp, out_cap,
                       (const uint8_t **)frame->data, frame->nb_samples);
}

// Decode one access unit into interleaved mono s16 at 48 kHz. `out_cap` is the
// sample capacity. Returns the number of samples written, or -1 on error.
int eld_decode(void *handle, const uint8_t *au, int au_len, int16_t *out, int out_cap) {
    EldDec *d = (EldDec *)handle;
    if (!d || !au || au_len <= 0 || au_len > 255 || !out || out_cap <= 0) return -1;
    // libavcodec needs zero padding beyond the packet; a borrowed Rust slice
    // does not provide it. av_new_packet allocates and initializes that padding.
    av_packet_unref(d->pkt);
    if (av_new_packet(d->pkt, au_len) < 0) return -1;
    memcpy(d->pkt->data, au, au_len);
    int sent = avcodec_send_packet(d->ctx, d->pkt);
    av_packet_unref(d->pkt);
    if (sent < 0) return -1;

    int total = 0;
    int received;
    while ((received = avcodec_receive_frame(d->ctx, d->frame)) == 0) {
        int got = eld_convert_frame(d, out + total, out_cap - total);
        av_frame_unref(d->frame);
        if (got < 0) return -1;
        total += got;
    }
    if (received != AVERROR(EAGAIN) && received != AVERROR_EOF) return -1;
    return total;
}

// The decoder's output sample rate (learned after the first decoded frame), or 0.
int eld_in_rate(void *handle) {
    EldDec *d = (EldDec *)handle;
    return d ? d->in_rate : 0;
}

void eld_close(void *handle) {
    EldDec *d = (EldDec *)handle;
    if (!d) return;
    if (d->swr) swr_free(&d->swr);
    av_frame_free(&d->frame);
    av_packet_free(&d->pkt);
    avcodec_free_context(&d->ctx);
    free(d);
}
