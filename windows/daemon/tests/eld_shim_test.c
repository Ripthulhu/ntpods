// Exercise the production conversion path without recording anyone's voice.
#include <assert.h>
#include <stdio.h>
#include "../src/eld_shim.c"

static EldDec *fixture(void) {
    const uint8_t asc[] = {0xf8, 0xe6, 0x30, 0x00};
    EldDec *d = eld_open(asc, sizeof(asc), OUT_RATE);
    assert(d);
    d->frame->format = AV_SAMPLE_FMT_FLTP;
    d->frame->sample_rate = OUT_RATE;
    d->frame->nb_samples = FRAME_SAMPLES;
    av_channel_layout_default(&d->frame->ch_layout, 1);
    assert(av_frame_get_buffer(d->frame, 0) == 0);
    return d;
}

static void invalid_frames_leave_output_untouched(void) {
    for (int kind = 0; kind < 11; kind++) {
        EldDec *d = fixture();
        float *samples = (float *)d->frame->data[0];
        memset(samples, 0, FRAME_SAMPLES * sizeof(float));
        int16_t out[1024];
        for (int i = 0; i < 1024; i++) out[i] = 1234;
        switch (kind) {
        case 0: samples[100] = NAN; break;
        case 1: samples[100] = INFINITY; break;
        case 2: samples[100] = -INFINITY; break;
        case 3: d->frame->flags |= AV_FRAME_FLAG_CORRUPT; break;
        case 4: d->frame->flags |= AV_FRAME_FLAG_DISCARD; break;
        case 5: d->frame->decode_error_flags = FF_DECODE_ERROR_INVALID_BITSTREAM; break;
        case 6: d->frame->nb_samples = 960; break;
        case 7: d->frame->sample_rate = 64000; break;
        case 8: d->frame->format = AV_SAMPLE_FMT_S16; break;
        case 9: d->frame->ch_layout.nb_channels = 2; break;
        case 10: break; // valid frame, insufficient output space
        }
        assert(eld_convert_frame(d, out, kind == 10 ? 1 : 1024) == -1);
        for (int i = 0; i < 1024; i++) assert(out[i] == 1234);
        eld_close(d);
    }
}

static void shared_decoder_history_is_not_modified(void) {
    EldDec *d = fixture();
    float *samples = (float *)d->frame->data[0];
    for (int i = 0; i < FRAME_SAMPLES; i++) samples[i] = 0.1f;
    AVFrame *history = av_frame_clone(d->frame);
    assert(history);
    assert(!av_frame_is_writable(d->frame));
    int16_t out[1024];
    int count = eld_convert_frame(d, out, 1024);
    assert(count > 0 && count <= 360);
    for (int i = 0; i < FRAME_SAMPLES; i++) {
        assert(((float *)history->data[0])[i] == 0.1f);
        assert(fabsf(((float *)d->frame->data[0])[i] - tanhf(0.3f)) < 0.00001f);
    }
    av_frame_free(&history);
    eld_close(d);
}

static void conversion_is_bounded_and_keeps_the_existing_clock(void) {
    for (int format = AV_SAMPLE_FMT_FLT; format <= AV_SAMPLE_FMT_FLTP;
         format += AV_SAMPLE_FMT_FLTP - AV_SAMPLE_FMT_FLT) {
        EldDec *d = fixture();
        d->frame->format = format;
        int total = 0;
        for (int frame = 0; frame < 100; frame++) {
            memset(d->frame->data[0], 0, FRAME_SAMPLES * sizeof(float));
            int16_t out[1026];
            for (int i = 0; i < 1026; i++) out[i] = 1234;
            int count = eld_convert_frame(d, out + 1, 1024);
            assert(count > 0 && count <= 360);
            total += count;
            assert(out[0] == 1234 && out[1025] == 1234);
            for (int i = 1; i <= count; i++) assert(out[i] == 0);
            for (int i = count + 1; i < 1026; i++) assert(out[i] == 1234);
        }
        assert(total > 35900 && total <= 36000);
        eld_close(d);
    }
}

int main(void) {
    invalid_frames_leave_output_untouched();
    shared_decoder_history_is_not_modified();
    conversion_is_bounded_and_keeps_the_existing_clock();
    puts("ELD conversion: invalid frames, shared history, output bounds and clock passed");
    return 0;
}
