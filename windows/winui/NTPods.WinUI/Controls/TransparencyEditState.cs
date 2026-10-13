using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class TransparencyEditState
{
    private string _deviceId = "";
    private bool _pending;
    private string? _errorAtRequest;
    private TransparencySettings? _written;
    public TransparencySettings? Draft { get; private set; }
    public bool Dirty { get; private set; }
    public bool Requesting { get; private set; }

    public void Update(Snapshot s, bool available)
    {
        if (_deviceId != s.DeviceId || !available)
        {
            _deviceId = s.DeviceId;
            Draft = available ? s.Transparency : null;
            Dirty = Requesting = _pending = false;
            return;
        }
        if (s.TransparencyPending || s.Transparency == _written && _written is not null ||
            s.TransparencyError != _errorAtRequest)
            Requesting = false;
        if (_pending && !s.TransparencyPending || !Dirty && !Requesting && !s.TransparencyPending)
        {
            Draft = s.Transparency;
            Dirty = false;
        }
        _pending = s.TransparencyPending;
    }

    public void Edit(TransparencySettings value)
    {
        if (!value.IsValid) return;
        Draft = value;
        Dirty = true;
    }

    public void Begin(Snapshot s, bool writing)
    {
        Dirty = false;
        Requesting = true;
        _errorAtRequest = s.TransparencyError;
        _written = writing ? Draft : null;
    }

    public void Cancel() => Requesting = false;
}
