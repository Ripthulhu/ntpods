#include <assert.h>
#include <stdio.h>
#include "../Common/RenderDiagnostics.h"

int main()
{
    RenderDiagnostics state = {};
    RenderVolumeState volume;
    RenderDiagnosticVolume(&state, &volume, 10);
    RenderDiagnosticReset(&state, 7);
    assert(state.Version == 1 && state.Generation == 7);
    assert(state.VolumeFlags == RenderDiagnosticMinimum && state.Level == RenderVolumeMinimum);
    RenderDiagnosticPacket(&state, 20, state.VolumeFlags, true, 0, false);
    assert(state.Packets == 1 && state.MissingPackets == 1 && state.GatedPackets == 1);
    assert(state.UninitializedPackets == 1 && state.UnsubscribedPackets == 1);
    assert(state.MinimumPackets == 1 && state.MutedPackets == 0);
    assert(state.SourceZeroPackets == 0 && state.SourceNonzeroPackets == 0);
    assert(state.FirstPacketHns == 20 && state.FirstSourceNonzeroHns == 0);

    RenderAssignVolume(&volume, -10 * 65536, false);
    RenderDiagnosticVolume(&state, &volume, 30);
    RenderDiagnosticPacket(&state, 40, state.VolumeFlags, false, 8, true);
    assert(state.FirstSourceNonzeroHns == 40 && state.FirstForwardedNonzeroHns == 0);
    assert(state.SourceNonzeroPackets == 1 && state.GatedPackets == 2);
    RenderAssignVolume(&volume, volume.Level, true);
    RenderDiagnosticVolume(&state, &volume, 50);
    volume.Subscribed = true;
    RenderDiagnosticVolume(&state, &volume, 60);
    assert(state.InitializedHns == 50 && state.SubscribedHns == 60);
    RenderDiagnosticPacket(&state, 70, state.VolumeFlags, false, 8, false);
    assert(state.SourceZeroPackets == 1 && state.FirstForwardedNonzeroHns == 0);
    RenderDiagnosticPacket(&state, 80, state.VolumeFlags, false, 8, true);
    assert(state.FirstForwardedNonzeroHns == 80 && state.GatedPackets == 2);
    RenderDiagnosticPacket(&state, 90, state.VolumeFlags, false, 0, false); // Empty EOS is not a missing packet.
    assert(state.MissingPackets == 1 && state.SourceZeroPackets == 1);

    RenderAssignMute(&volume, true);
    RenderDiagnosticVolume(&state, &volume, 100);
    RenderDiagnosticPacket(&state, 110, state.VolumeFlags, false, 8, true);
    assert(state.MutedPackets == 1 && state.GatedPackets == 3);
    RenderDiagnosticReset(&state, 8);
    assert(state.Generation == 8 && state.Packets == 0 && state.GatedPackets == 0);
    assert(state.FirstPacketHns == 0 && state.FirstSourceNonzeroHns == 0 && state.FirstForwardedNonzeroHns == 0);
    assert(state.InitializedHns == 50 && state.SubscribedHns == 60 && state.Level == volume.Level);
    RenderVolumeDisconnect(&volume);
    RenderDiagnosticVolume(&state, &volume, 120);
    assert(state.InitializedHns == 0 && state.SubscribedHns == 0);
    volume.Subscribed = true;
    RenderDiagnosticVolume(&state, &volume, 130);
    RenderAssignVolume(&volume, volume.Level, true);
    RenderDiagnosticVolume(&state, &volume, 140);
    assert(state.InitializedHns == 140 && state.SubscribedHns == 130);
    RenderDiagnosticVolume(&state, &volume, 150);
    assert(state.InitializedHns == 140 && state.SubscribedHns == 130);
    state.Packets = ~0ull;
    RenderDiagnosticPacket(&state, 160, state.VolumeFlags, true, 0, false);
    assert(state.Packets == ~0ull && state.MissingPackets == 1);

    unsigned char samples[16] = {};
    assert(!RenderDiagnosticNonzero(samples, 16, 8));
    samples[3] = samples[7] = samples[11] = samples[15] = 0x80; // Float -0 is silence.
    assert(!RenderDiagnosticNonzero(samples, 16, 8));
    samples[0] = 1; // Smallest subnormal is still a nonzero sample.
    assert(RenderDiagnosticNonzero(samples, 16, 8));
    samples[0] = 0;
    samples[2] = 0xc0;
    samples[3] = 0x7f; // NaN is nonzero data, not an amplitude measurement.
    assert(RenderDiagnosticNonzero(samples, 16, 8));
    memset(samples, 0, sizeof(samples));
    assert(!RenderDiagnosticNonzero(samples, 16, 4));
    samples[15] = 0x80; // PCM16 negative values must not use float signed-zero handling.
    assert(RenderDiagnosticNonzero(samples, 16, 4));
    assert(!RenderDiagnosticNonzero(nullptr, 8, 8));
    assert(!RenderDiagnosticNonzero(samples, 7, 8));
    assert(!RenderDiagnosticNonzero(samples, 16, 16));
    assert(!RenderDiagnosticNonzero(samples, RENDER_PACKET_MAX_BYTES + 8, 8));
    puts("render diagnostics tests passed");
}
