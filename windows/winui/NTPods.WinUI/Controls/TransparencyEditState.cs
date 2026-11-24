using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class TransparencyEditState
{
    private string _deviceId = "";
    private bool _pending;
    private string? _errorAtRequest;
    private TransparencySettings? _written;
    private ulong? _session;
    public TransparencySettings? Draft { get; private set; }
    public TransparencySettings? Baseline { get; private set; }
    public bool Dirty { get; private set; }
    public bool Requesting { get; private set; }
    public bool Sending => Requesting || _written is not null;

    public void Update(Snapshot s, bool available)
    {
        var current = s.Transparency;
        if (_deviceId != s.DeviceId || _session != s.HeadphoneAccommodations?.Session || !available || current is null)
        {
            _deviceId = s.DeviceId;
            _session = s.HeadphoneAccommodations?.Session;
            Draft = Baseline = available ? current : null;
            Dirty = Requesting = _pending = false;
            _written = null;
            _errorAtRequest = s.TransparencyError;
            return;
        }
        if (_written is not null)
        {
            if (!s.TransparencyPending && s.TransparencyError is null && current.SameValues(_written))
            {
                // A newer gesture survives acknowledgement of the preceding write.
                Baseline = current;
                if (!Dirty) Draft = current;
                Dirty = Draft?.SameValues(current) == false;
                Requesting = _pending = false;
                _written = null;
                return;
            }
            if (s.TransparencyError != _errorAtRequest || _pending && !s.TransparencyPending ||
                !current.SameValues(Baseline) && !current.SameValues(_written))
            {
                Draft = Baseline = current;
                Dirty = Requesting = _pending = false;
                _written = null;
                return;
            }
            if (s.TransparencyPending) Requesting = false;
            _pending = s.TransparencyPending;
            return;
        }
        if (!current.SameValues(Baseline))
        {
            Draft = Baseline = current;
            Dirty = Requesting = _pending = false;
            return;
        }
        if (s.TransparencyPending || s.Transparency?.SameValues(_written) == true ||
            s.TransparencyError != _errorAtRequest)
            Requesting = false;
        if (_pending && !s.TransparencyPending || !Dirty && !Requesting && !s.TransparencyPending)
        {
            Draft = Baseline = s.Transparency;
            Dirty = false;
        }
        _pending = s.TransparencyPending;
    }

    public void Edit(TransparencySettings value)
    {
        if (!value.IsValid) return;
        Draft = value;
        Dirty = !value.SameValues(_written ?? Baseline);
    }

    public void Begin(Snapshot s, bool writing)
    {
        Dirty = false;
        Requesting = true;
        _errorAtRequest = s.TransparencyError;
        _written = writing ? Draft : null;
    }

    public void Cancel()
    {
        Requesting = Dirty = _pending = false;
        _written = null;
        Draft = Baseline;
    }
}
