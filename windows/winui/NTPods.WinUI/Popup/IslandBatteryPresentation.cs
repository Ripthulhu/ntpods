using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Popup;

internal static class IslandBatteryPresentation
{
    private const ulong CaseGraceMs = 90_000;

    public static bool IsCurrentForDisplay(Snapshot snapshot, BatteryPart part, DateTimeOffset utcNow) =>
        BatteryPresentation.IsLive(snapshot, part) ||
        part == BatteryPart.Case && CaseGraceExpiresAt(snapshot, utcNow).HasValue;

    public static DateTimeOffset? CaseGraceExpiresAt(Snapshot snapshot, DateTimeOffset utcNow)
    {
        // The case may go silent while the buds remain available; its charging state stays strict.
        if (string.IsNullOrEmpty(snapshot.DeviceId) || snapshot.BatterySource is not ("aap" or "ble") ||
            !snapshot.BatteryLive || BatteryPresentation.Reading(snapshot, BatteryPart.Case) is null ||
            snapshot.BatteryFreshness?.Case is not { Present: false, TimestampMs: > 0 } observation)
            return null;
        ulong stamp = observation.TimestampMs.Value;
        long now = utcNow.ToUnixTimeMilliseconds();
        if (now < 0 || stamp > (ulong)now || (ulong)now - stamp >= CaseGraceMs ||
            stamp > 253_402_300_799_999UL - CaseGraceMs)
            return null;
        return DateTimeOffset.FromUnixTimeMilliseconds((long)(stamp + CaseGraceMs));
    }

    public static string FreshnessText(Snapshot snapshot, BatteryPart part, DateTimeOffset utcNow) =>
        IsCurrentForDisplay(snapshot, part, utcNow)
            ? snapshot.Connected ? "" : Localize.Get("Battery.LiveHelp")
            : BatteryPresentation.FreshnessText(snapshot, part);

    public static string SharedFreshnessText(Snapshot snapshot, bool headphone, DateTimeOffset utcNow)
    {
        BatteryPart[] parts = headphone ? [BatteryPart.Headphone] : [BatteryPart.Left, BatteryPart.Right, BatteryPart.Case];
        var known = parts.Where(part => BatteryPresentation.Reading(snapshot, part).HasValue).ToArray();
        if (known.Length == 0 || known.Any(part => IsCurrentForDisplay(snapshot, part, utcNow))) return "";
        var labels = known.Select(part => FreshnessText(snapshot, part, utcNow)).Distinct().ToArray();
        return labels.Length == 1 ? labels[0] : "";
    }
}
