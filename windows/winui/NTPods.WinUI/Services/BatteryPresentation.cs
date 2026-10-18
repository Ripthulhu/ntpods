using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Services;

internal enum BatteryPart { Left, Right, Case, Headphone }

internal static class BatteryPresentation
{
    public static byte? Present(byte? value) => value is <= 100 ? value : null;

    public static byte? LowestReading(params byte?[] values)
    {
        byte? lowest = null;
        foreach (var value in values)
            if (Present(value) is byte level && (lowest is null || level < lowest))
                lowest = level;
        return lowest;
    }

    public static bool HasData(Snapshot snapshot) => LowestReading(snapshot.Battery.Left,
        snapshot.Battery.Right, snapshot.Battery.Case, snapshot.Battery.Headphone) is not null;

    // Older daemons did not carry freshness metadata. New sources always own their
    // live flag, independently of the audio connection and its cached readings.
    public static bool IsLive(Snapshot snapshot) => snapshot.BatterySource switch
    {
        "" => snapshot.Connected && !snapshot.LinkStale,
        "aap" or "ble" => snapshot.BatteryLive,
        _ => false,
    };

    public static byte? Reading(Snapshot s, BatteryPart part) => Present(part switch
    {
        BatteryPart.Left => s.Battery.Left, BatteryPart.Right => s.Battery.Right,
        BatteryPart.Case => s.Battery.Case, _ => s.Battery.Headphone,
    });

    private static BatteryObservation? Observation(Snapshot s, BatteryPart part) => part switch
    {
        BatteryPart.Left => s.BatteryFreshness?.Left, BatteryPart.Right => s.BatteryFreshness?.Right,
        BatteryPart.Case => s.BatteryFreshness?.Case, _ => s.BatteryFreshness?.Headphone,
    };

    public static bool IsLive(Snapshot s, BatteryPart part) => Reading(s, part).HasValue && IsLive(s)
        && (s.BatteryFreshness is null || Observation(s, part) is { Live: true, TimestampMs: not null, Present: not false });

    public static bool ArtworkIsAvailable(Snapshot s, BatteryPart part) =>
        (part != BatteryPart.Case && s.Connected && Observation(s, part)?.Present != false) || IsLive(s, part);

    public static bool IsCharging(Snapshot s, BatteryPart part) => IsLive(s, part) && (part switch
    {
        BatteryPart.Left => s.Battery.LeftCharging, BatteryPart.Right => s.Battery.RightCharging,
        BatteryPart.Case => s.Battery.CaseCharging, _ => s.Battery.HeadphoneCharging,
    });

    public static bool IsCharging(Snapshot s) => Enum.GetValues<BatteryPart>().Any(part => IsCharging(s, part));

    public static byte? LiveEarbudReading(Snapshot s) => LowestReading(
        IsLive(s, BatteryPart.Left) ? s.Battery.Left : null,
        IsLive(s, BatteryPart.Right) ? s.Battery.Right : null,
        IsLive(s, BatteryPart.Headphone) ? s.Battery.Headphone : null);

    public static BatteryPart? EarbudSummaryPart(Snapshot s) =>
        new[] { BatteryPart.Left, BatteryPart.Right, BatteryPart.Headphone }
            .Where(part => Reading(s, part).HasValue)
            .OrderByDescending(part => IsLive(s, part))
            .ThenBy(part => Reading(s, part))
            .Select(part => (BatteryPart?)part).FirstOrDefault();

    public static string ReadingText(Snapshot s, BatteryPart part, string label)
    {
        if (Reading(s, part) is not byte value) return "";
        string text = $"{label} {value}%".Trim();
        return !IsLive(s) || IsLive(s, part) ? text : $"{text} ({Localize.Get("Battery.LastKnown")})";
    }

    public static string StatusKey(Snapshot snapshot) => snapshot.Connected
        ? snapshot.LinkStale ? "Status_ControlUnavailable" : "Status_Connected"
        : IsLive(snapshot) && HasData(snapshot)
            ? IsCharging(snapshot) ? "Battery.NearbyCharging" : "Battery.Nearby"
            : "Status_Disconnected";

    private static DateTimeOffset? LastSeen(ulong? timestamp) => timestamp is ulong stamp
        && stamp <= 253_402_300_799_999UL ? DateTimeOffset.FromUnixTimeMilliseconds((long)stamp) : null;

    public static DateTimeOffset? LastSeen(Snapshot snapshot) => LastSeen(snapshot.BatteryTimestampMs);

    public static string FreshnessText(Snapshot s, BatteryPart part)
    {
        if (Reading(s, part) is null) return Localize.Get("Tray_NoBatteryData");
        if (!IsLive(s, part))
            return LastSeen(s.BatteryFreshness is null ? s.BatteryTimestampMs : Observation(s, part)?.TimestampMs) is { } seen
                ? Localize.Get("Battery.LastSeen", seen.ToLocalTime().ToString("g"))
                : Localize.Get("Battery.LastKnown");
        return s.Connected ? "" : Localize.Get("Battery.LiveHelp");
    }

    public static string FreshnessKey(Snapshot snapshot) => !HasData(snapshot) ? "Tray_NoBatteryData"
        : !IsLive(snapshot) ? "Battery.LastKnown"
        : !snapshot.Connected ? "Battery.LiveHelp" : "";

    public static string FreshnessText(Snapshot snapshot)
    {
        if (HasData(snapshot) && !IsLive(snapshot) && LastSeen(snapshot) is { } seen)
            return Localize.Get("Battery.LastSeen", seen.ToLocalTime().ToString("g"));
        string key = FreshnessKey(snapshot);
        return key.Length == 0 ? "" : Localize.Get(key);
    }

    public static string SharedFreshnessText(Snapshot snapshot, bool headphone)
    {
        BatteryPart[] parts = headphone ? [BatteryPart.Headphone] : [BatteryPart.Left, BatteryPart.Right, BatteryPart.Case];
        var known = parts.Where(part => Reading(snapshot, part).HasValue).ToArray();
        if (known.Length == 0 || known.Any(part => IsLive(snapshot, part))) return "";
        var labels = known.Select(part => FreshnessText(snapshot, part)).Distinct().ToArray();
        return labels.Length == 1 ? labels[0] : "";
    }
}
