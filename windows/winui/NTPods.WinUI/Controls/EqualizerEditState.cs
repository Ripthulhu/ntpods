using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class EqualizerEditState
{
    private readonly LiveSettingEdit<Equalizer> _live = new();
    public bool Dirty => _live.Desired is not null;
    public Equalizer? Requested => _live.Requested;
    public Equalizer? Displayed => _live.Displayed;
    public void Edit(Equalizer value) => _live.Edit(value.IsValid ? value : null);
    public Equalizer? Take() => _live.Take();
    public void Update(Snapshot snapshot, bool available, ulong clientEpoch = 0) =>
        _live.Update(LiveSettingScope.For(snapshot, clientEpoch), available,
            snapshot.Eq, snapshot.EqPending, snapshot.DeviceSettingsError);
    public void EndRequest() => _live.EndRequest();
    public void Cancel() => _live.Cancel();
}
