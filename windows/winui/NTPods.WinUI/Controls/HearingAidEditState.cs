using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class HearingAidEditState
{
    private string _deviceId = "";
    private HearingAidSettings? _baseline;
    public HearingAidSettings? Baseline => _baseline is { } value ? Copy(value) : null;
    public HearingAidSettings? Draft { get; private set; }
    public bool Dirty { get; private set; }
    public bool InvalidCurve { get; private set; }
    public bool Editable { get; private set; }
    public bool CanApply => Editable && Draft is { IsValid: true } && !InvalidCurve;
    public float Amplification { get; private set; } = 0.5f;
    public float Balance { get; private set; }

    public bool Update(Snapshot snapshot, bool available)
    {
        bool validRead = snapshot.HearingAidError is null && snapshot.HearingAidSettings is { IsValid: true };
        if (_deviceId != snapshot.DeviceId || !available || !validRead)
        {
            Reset();
            _deviceId = snapshot.DeviceId;
        }
        if (!available || !validRead || snapshot.HearingAidSettings is not { } settings) return true;
        bool editable = snapshot.HearingAidEditable &&
            settings.LeftAmplification is >= -1 and <= 1 && settings.RightAmplification is >= -1 and <= 1;
        bool editabilityChanged = Editable != editable;
        Editable = editable;
        if (_baseline?.SameValues(settings) == true && !editabilityChanged) return false;
        _baseline = Copy(settings);
        Draft = Copy(settings);
        Amplification = (settings.LeftAmplification + settings.RightAmplification + 2) / 4;
        Balance = (settings.RightAmplification - settings.LeftAmplification) / 2;
        Dirty = InvalidCurve = false;
        return true;
    }

    public bool EditCurve(double[] left, double[] right)
    {
        if (!Editable || Draft is null) return false;
        InvalidCurve = left.Length != 8 || right.Length != 8 ||
            left.Concat(right).Any(value => !double.IsFinite(value) || value is < 0 or > 90);
        if (InvalidCurve || Draft is not { } value) return false;
        return Edit(value with { LeftEq = left.Select(v => (float)v).ToArray(), RightEq = right.Select(v => (float)v).ToArray() });
    }

    public bool EditAmplification(float amplification) => float.IsFinite(amplification) && amplification is >= 0 and <= 1 &&
        EditAmplificationAndBalance(amplification, Balance);

    public bool EditBalance(float balance) => float.IsFinite(balance) && balance is >= -1 and <= 1 &&
        EditAmplificationAndBalance(Amplification, balance);

    private bool EditAmplificationAndBalance(float amplification, float balance)
    {
        if (Draft is not { } value || !Edit(value with {
            LeftAmplification = Math.Clamp(amplification * 2 - 1 - balance, -1, 1),
            RightAmplification = Math.Clamp(amplification * 2 - 1 + balance, -1, 1)
        })) return false;
        // Clipping one ear must not change the other slider's requested value.
        Amplification = amplification;
        Balance = balance;
        return true;
    }

    public bool EditTone(float tone) => Draft is { } value && Edit(value with { LeftTone = tone, RightTone = tone });
    public bool EditConversationBoost(bool on) => Draft is { } value &&
        Edit(value with { LeftConversationBoost = on, RightConversationBoost = on });

    private bool Edit(HearingAidSettings value)
    {
        if (!CanApply || !value.IsValid) return false;
        Draft = value;
        Dirty = true;
        return true;
    }

    public void Reset()
    {
        _baseline = Draft = null;
        Amplification = 0.5f;
        Balance = 0;
        Dirty = InvalidCurve = Editable = false;
    }

    private static HearingAidSettings Copy(HearingAidSettings value) =>
        value with { LeftEq = [.. value.LeftEq], RightEq = [.. value.RightEq] };
}
