using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Popup;

internal sealed class IslandPresence
{
    private string _deviceId = "";
    private bool _connected;
    private bool _nearby;

    public bool Update(Snapshot snapshot, bool sessionReady = true, bool allowPresentation = true)
    {
        // A lost IPC pipe supplies an empty snapshot, not an AirPods disconnect.
        if (!sessionReady) return false;
        if (!string.Equals(_deviceId, snapshot.DeviceId, StringComparison.OrdinalIgnoreCase))
        {
            _deviceId = snapshot.DeviceId;
            _connected = false;
            _nearby = false;
        }
        bool connected = allowPresentation && snapshot.AudioOutputReady;
        bool nearby = allowPresentation && !snapshot.AudioOutputReady && BatteryPresentation.IsLive(snapshot) && BatteryPresentation.HasData(snapshot);
        bool show = connected && !_connected || nearby && !_nearby;
        _connected = connected;
        _nearby = nearby;
        return show;
    }

    public void OnPairedLidClose()
    {
        _connected = false;
        _nearby = false;
    }
}
