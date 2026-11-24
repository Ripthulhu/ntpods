using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class AccommodationsEditState
{
    private string _deviceId = "";
    private string _preferencesId = "";
    private bool _pending;
    private bool _editable;
    private bool _rememberedPhone = true, _rememberedMedia = true;
    private HeadphoneAccommodationsState? _atRequest;
    private HeadphoneAccommodationsState? _written;
    private (ulong Id, ulong ClientEpoch)? _request;
    private readonly Func<string, (bool Phone, bool Media)?>? _readChoices;
    private readonly Action<string, bool, bool>? _saveChoices;
    public HeadphoneAccommodationsState? Baseline { get; private set; }
    public HeadphoneAccommodationsState? Draft { get; private set; }
    public bool Dirty { get; private set; }
    public bool Requesting { get; private set; }
    public bool Sending => Requesting || _written is not null;
    public bool CanEdit => _editable || _written is not null && Baseline?.Editable == true;
    public bool Enabled => Draft?.Phone == true || Draft?.Media == true;
    public bool? PhoneChoice => Draft?.Phone is null ? null : Enabled ? Draft.Phone : _rememberedPhone;
    public bool? MediaChoice => Draft?.Media is null ? null : Enabled ? Draft.Media : _rememberedMedia;

    public AccommodationsEditState(Func<string, (bool Phone, bool Media)?>? readChoices = null,
        Action<string, bool, bool>? saveChoices = null)
    { _readChoices = readChoices; _saveChoices = saveChoices; }

    public (HeadphoneAccommodationsState? Settings, bool Enabled, bool? Phone, bool? Media) View(HeadphoneAccommodationsState? report) =>
        !Sending && report is { Editable: false, Requested: null, Phone: not null, Media: not null }
            ? (report, report.Phone == true || report.Media == true, report.Phone, report.Media)
            : (Draft, Enabled, PhoneChoice, MediaChoice);

    private static HeadphoneAccommodationsState? Selection(HeadphoneAccommodationsState? state) =>
        state?.Requested is { } sent ? state with { Phone = sent.Phone, Media = sent.Media, Preset = sent.Preset, Audiogram = sent.Audiogram } : state;

    private void Remember(bool phone, bool media)
    {
        _rememberedPhone = phone;
        _rememberedMedia = media;
        _saveChoices?.Invoke(_preferencesId, phone, media);
    }

    public void Update(Snapshot snapshot, bool available, string? preferenceDeviceId = null)
    {
        var value = snapshot.HeadphoneAccommodations;
        string preferenceKey = preferenceDeviceId ?? (snapshot.DeviceId.StartsWith("dongle:", StringComparison.Ordinal)
            ? $"unidentified:{snapshot.DeviceId}:{value?.Session}" : snapshot.DeviceId);
        _editable = available && value?.Editable == true && !value.Pending;
        bool changedConnection = _deviceId != snapshot.DeviceId || _preferencesId != preferenceKey || Baseline?.Session != value?.Session || !available;
        bool changedReport = Baseline?.Revision != value?.Revision || Baseline?.EditVersion != value?.EditVersion;
        if (!changedConnection && _written is not null)
        {
            if (value?.Pending != true && value?.Error is null && value?.Result is "sent" or "confirmed" &&
                changedReport && SameSelection(Selection(value), _written))
            {
                Baseline = value;
                if (!Dirty) Draft = Selection(value);
                Dirty = !SameSelection(Draft, Selection(value));
                Requesting = _pending = false;
                _written = null;
                return;
            }
            if (value?.Pending != true && (changedReport || _pending || value?.Error != _atRequest?.Error))
            {
                Baseline = value;
                Draft = Selection(value);
                Dirty = Requesting = _pending = false;
                _written = null;
                return;
            }
            if (value?.Pending == true) Requesting = false;
            _pending = value?.Pending == true;
            return;
        }
        if (changedConnection || changedReport || Baseline?.AudiogramAvailable != value?.AudiogramAvailable)
        {
            if (_preferencesId != preferenceKey)
            {
                var saved = _readChoices?.Invoke(preferenceKey);
                _rememberedPhone = saved?.Phone ?? true;
                _rememberedMedia = saved?.Media ?? true;
            }
            _deviceId = snapshot.DeviceId;
            _preferencesId = preferenceKey;
            Baseline = available ? value : null;
            Draft = Selection(Baseline);
            Dirty = Requesting = _pending = false;
            _written = null;
            if (Draft is { Phone: not null, Media: not null } && Enabled)
                Remember(Draft.Phone.Value, Draft.Media.Value);
            return;
        }
        if (value?.Pending == true || value != _atRequest) Requesting = false;
        if (_pending && value?.Pending != true || !Dirty && !Requesting && value?.Pending != true)
        {
            Baseline = value;
            Draft = Selection(value);
            Dirty = false;
            if (Draft is { Phone: not null, Media: not null } && Enabled)
                Remember(Draft.Phone.Value, Draft.Media.Value);
        }
        _pending = value?.Pending == true;
    }

    public void Enable(bool enabled)
    {
        if (Draft is not { } value) return;
        if (!enabled)
        {
            if (Enabled) Remember(value.Phone == true, value.Media == true);
            Edit(value with { Phone = false, Media = false });
        }
        else
        {
            if (!_rememberedPhone && !_rememberedMedia) return;
            Edit(value with { Phone = _rememberedPhone, Media = _rememberedMedia });
        }
    }

    public void Destinations(bool? phone, bool? media)
    {
        if (Draft is not { } value || phone is not bool p || media is not bool m) return;
        bool enabled = Enabled;
        Remember(p, m);
        if (enabled) Edit(value with { Phone = p, Media = m });
    }

    public void Edit(HeadphoneAccommodationsState value)
    {
        if (!value.IsValid) return;
        Draft = value;
        Dirty = !SameSelection(value, _written ?? Selection(Baseline));
    }

    private static bool SameSelection(HeadphoneAccommodationsState? left, HeadphoneAccommodationsState? right) =>
        left?.Phone == right?.Phone && left?.Media == right?.Media && left?.Preset == right?.Preset && left?.Audiogram == right?.Audiogram;

    public SetHeadphoneAccommodationsCmd? Command(ulong requestId) => _editable && !Sending && Dirty && Baseline is { Editable: true } baseline && Selection(baseline) is { } original && Draft is { } draft && (!draft.Audiogram || original.Audiogram || baseline.AudiogramAvailable)
        ? new() { RequestId = requestId, DeviceId = _deviceId, Session = baseline.Session, Revision = baseline.Revision,
            EditVersion = baseline.EditVersion,
            Audiogram = draft.Audiogram && !original.Audiogram,
            Phone = draft.Phone != original.Phone ? draft.Phone : null,
            Media = draft.Media != original.Media ? draft.Media : null,
            Preset = draft.Preset != original.Preset ? draft.Preset : null } : null;

    public void Begin(Snapshot snapshot, ulong requestId = 0, ulong clientEpoch = 0)
    {
        _request = (requestId, clientEpoch);
        _atRequest = snapshot.HeadphoneAccommodations; _written = Draft; Dirty = false; Requesting = true;
    }
    public bool FailRequest(DaemonEvent.CommandResult result, ulong clientEpoch)
    {
        if (!Sending || result.Ok || result.Command != "set_headphone_accommodations" ||
            _request != (result.RequestId, clientEpoch)) return false;
        Cancel();
        return true;
    }
    public void UseCachedReport(Snapshot snapshot)
    {
        Baseline = snapshot.HeadphoneAccommodations;
        Draft = Selection(Baseline);
        Dirty = Requesting = _pending = false;
        _written = null;
    }
    public void Cancel() { Requesting = Dirty = _pending = false; _written = null; _request = null; Draft = Selection(Baseline); }
}
