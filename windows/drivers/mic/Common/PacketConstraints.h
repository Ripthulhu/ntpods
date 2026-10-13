#pragma once

inline KSAUDIO_PACKETSIZE_CONSTRAINTS2 AudioPacketConstraints(ULONG alignment, ULONG maximum)
{
    KSAUDIO_PACKETSIZE_CONSTRAINTS2 constraints = {};
    // The RAW 5 ms quantum must exceed the transport minimum to be honored.
    constraints.MinPacketPeriodInHns = 40000;
    constraints.PacketSizeFileAlignment = alignment;
    constraints.MaxPacketSizeInBytes = maximum;
    constraints.NumProcessingModeConstraints = 1;
    // Express the quantum as time so mono and stereo use the same period.
    constraints.ProcessingModeConstraints[0] = {{STATIC_AUDIO_SIGNALPROCESSINGMODE_RAW}, 0, 50000};
    return constraints;
}
