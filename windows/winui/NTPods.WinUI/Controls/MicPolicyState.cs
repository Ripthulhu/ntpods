using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

internal sealed class MicPolicyState
{
    private Snapshot _snapshot = new();
    public (string Mode, uint Delay)? PendingPolicy { get; private set; }
    public bool? PendingManual { get; private set; }
    public string Mode => PendingPolicy?.Mode ?? _snapshot.MicMode;
    public uint Delay => PendingPolicy?.Delay ?? _snapshot.MicStopDelaySecs;

    public void Update(Snapshot snapshot)
    {
        if (_snapshot.DeviceId != snapshot.DeviceId) Cancel();
        _snapshot = snapshot;
        if (PendingPolicy is { } policy && policy.Mode == snapshot.MicMode && policy.Delay == snapshot.MicStopDelaySecs)
            PendingPolicy = null;
        if (PendingManual == snapshot.MicManualActive) PendingManual = null;
    }

    public bool QueuePolicy(string mode, uint delay)
    {
        if (mode is not ("automatic" or "keep_active" or "manual") || delay > 3600) return false;
        PendingPolicy = (mode, delay);
        return true;
    }

    public void RequestManual(bool active) => PendingManual = active;
    public void Cancel() { PendingPolicy = null; PendingManual = null; }

    public static bool CanStart(Snapshot s) => DeviceModels.HighQualityMic(s) && s.MicMode == "manual" && s.AutoMode && s.MicAvailable &&
        s.Connected && !s.LinkStale && !s.MicManualActive && s.MicState is not ("starting" or "stopping" or "recovering");
    public static bool CanStop(Snapshot s) => s.MicMode == "manual" && s.MicManualActive;
}
