using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Popup;

internal sealed class IslandPresence
{
    private bool _connected;
    private bool _nearby;

    public bool Update(Snapshot snapshot, bool sessionReady = true)
    {
        // A lost IPC pipe supplies an empty snapshot, not an AirPods disconnect.
        if (!sessionReady) return false;
        bool nearby = !snapshot.Connected && BatteryPresentation.IsLive(snapshot) && BatteryPresentation.HasData(snapshot);
        bool show = snapshot.Connected && !_connected || nearby && !_nearby;
        _connected = snapshot.Connected;
        _nearby = nearby;
        return show;
    }
}
