using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class ListeningModeRequest
{
    private const byte ModeId = 0x0D;
    private const byte StrengthId = ControlId.AdaptiveNoiseStrength;
    private bool _pendingSeen;
    private (uint Mode, uint Strength)? _target;

    public byte? Id { get; private set; }
    public uint Value { get; private set; }
    public bool IsPending => Id is not null;
    public bool HasTarget => _target is not null;

    public uint DisplayMode(uint confirmed) => _target?.Mode ?? (Id == ModeId ? Value : confirmed);
    public uint DisplayStrength(uint confirmed) => _target is { Mode: 4 } target ? target.Strength :
        Id == StrengthId ? Value : confirmed;

    public void QueueGesture(uint mode, uint strength)
    {
        if (mode is >= 1 and <= 4 && strength <= 100) _target = (mode, strength);
    }

    public (byte Id, uint Value)? TakeNext(Snapshot state)
    {
        if (IsPending || _target is not { } target) return null;
        if (!ListeningModePolicy.CanContinueGesture(state) || !ListeningModePolicy.ModeSupported(state, target.Mode) ||
            target.Mode == 1 && !DeviceSettingStatus.AllowOffEnabled(state))
        {
            Cancel();
            return null;
        }
        if (!ListeningModePolicy.CanChooseMode(state, false)) return null;
        if (state.DeviceControls[ModeId] != target.Mode || state.AncUnconfirmed)
        {
            if (Begin(state, ModeId, target.Mode)) return (ModeId, Value);
            return null;
        }
        if (!state.AncUnconfirmed && target.Mode == 4 && Begin(state, StrengthId, target.Strength)) return (StrengthId, Value);
        _target = null;
        return null;
    }

    public bool Begin(Snapshot state, byte id, uint value)
    {
        if (!ListeningModePolicy.CanSend(state, IsPending, id, value)) return false;
        uint? current = id == StrengthId && state.AdaptiveStrengthRequested is uint sent ? sent :
            state.DeviceControls.TryGetValue(id, out var confirmed) ? confirmed : null;
        if (current == value && !(id == ModeId && state.AncUnconfirmed)) return false;
        Id = id;
        Value = value;
        _pendingSeen = false;
        return true;
    }

    public void Update(Snapshot state, bool newError)
    {
        if (!ListeningModePolicy.CanContinueGesture(state) || newError)
        {
            Cancel();
            return;
        }
        if (Id is not byte id) return;
        if (!state.DeviceControls.ContainsKey(id))
        {
            Cancel();
            return;
        }
        if (id == StrengthId && state.AdaptiveStrengthRequested is uint sent)
        {
            if (sent == Value) Complete();
            return;
        }
        if (state.DeviceControlsPending.TryGetValue(id, out var pending))
        {
            if (pending != Value) Cancel();
            else _pendingSeen = true;
            return;
        }
        if (state.DeviceControls[id] != Value || id == ModeId && state.AncUnconfirmed)
        {
            if (_pendingSeen) Cancel();
            return;
        }
        Complete();
    }

    private void Complete()
    {
        Id = null;
        _pendingSeen = false;
    }

    public void Cancel()
    {
        Complete();
        _target = null;
    }
}
