using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class EqualizerEditState
{
    private string _deviceId = "";
    public bool Dirty { get; private set; }
    public Equalizer? Requested { get; private set; }

    public void Edit() => Dirty = true;

    public void Begin(Equalizer requested)
    {
        Dirty = false;
        Requested = requested;
    }

    public void Update(Snapshot snapshot, bool available)
    {
        bool changedDevice = snapshot.DeviceId != _deviceId;
        _deviceId = snapshot.DeviceId;
        if (!available || changedDevice)
        {
            Dirty = false;
            Requested = null;
            return;
        }

        // Once the daemon replies, its pending/confirmed state owns the request.
        if (snapshot.EqPending is not null || snapshot.Eq == Requested ||
            !string.IsNullOrWhiteSpace(snapshot.DeviceSettingsError))
            Requested = null;
    }

    public void EndRequest() => Requested = null;
}
