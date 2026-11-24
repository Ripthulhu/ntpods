using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Services;

internal readonly record struct PairingBatteryLevels(
    byte? Left, byte? Right, byte? Case, byte? Headphone,
    bool LeftCharging, bool RightCharging, bool CaseCharging, bool HeadphoneCharging,
    ulong? NextExpiryMs, bool CaseLive);

internal sealed class PairingBatteryState
{
    private const ulong FreshMs = 90_000;
    private byte? _case;

    public void Clear() => _case = null;

    public byte? RetainCase(byte? freshCase) => _case = BatteryPresentation.Present(freshCase) ?? _case;

    public static string? DeviceId(string address)
    {
        string normalized = address.Length == 17 && Enumerable.Range(0, 17).All(i =>
            i % 3 == 2 ? address[i] == ':' : Uri.IsHexDigit(address[i]))
            ? address.Replace(":", "") : address;
        return normalized.Length == 12 && normalized.All(Uri.IsHexDigit)
            ? normalized.ToLowerInvariant() : null;
    }

    public static PairingBatteryLevels? Read(Snapshot snapshot, string address, ulong attemptStartedAtMs, ulong nowMs)
    {
        if (snapshot.BatterySource is not ("aap" or "ble") || DeviceId(address) is not { } deviceId ||
            !string.Equals(deviceId, snapshot.DeviceId, StringComparison.OrdinalIgnoreCase))
            return null;

        ulong? expiry = null;
        byte? Fresh(BatteryPart part, BatteryObservation? observation)
        {
            if (observation is not { Live: true, Present: not false, TimestampMs: ulong at } ||
                at <= attemptStartedAtMs || at > nowMs || nowMs - at >= FreshMs ||
                !BatteryPresentation.IsLive(snapshot, part)) return null;
            ulong end = at + FreshMs;
            expiry = expiry is { } earlier ? Math.Min(earlier, end) : end;
            return BatteryPresentation.Reading(snapshot, part);
        }

        byte? left = Fresh(BatteryPart.Left, snapshot.BatteryFreshness?.Left);
        byte? right = Fresh(BatteryPart.Right, snapshot.BatteryFreshness?.Right);
        byte? caseLevel = Fresh(BatteryPart.Case, snapshot.BatteryFreshness?.Case);
        bool caseLive = caseLevel is not null;
        byte? headphone = Fresh(BatteryPart.Headphone, snapshot.BatteryFreshness?.Headphone);
        return new(left, right, caseLevel, headphone,
            left is not null && snapshot.Battery.LeftCharging,
            right is not null && snapshot.Battery.RightCharging,
            caseLive && snapshot.Battery.CaseCharging,
            headphone is not null && snapshot.Battery.HeadphoneCharging, expiry, caseLive);
    }
}
