using System.Text.Json;
using System.Text.Json.Serialization;
using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Services;

internal sealed class DeviceModel
{
    public string[] Models { get; init; } = [];
    public string Name { get; init; } = "";
    public string Family { get; init; } = "airpods";
    public bool Headphone { get; init; }
    public bool Listening { get; init; }
    public bool Adaptive { get; init; }
    [JsonPropertyName("allow_off_option")] public bool AllowOffOption { get; init; }
    [JsonPropertyName("heart_rate")] public bool HeartRate { get; init; }
    [JsonPropertyName("hearing_aid")] public bool HearingAid { get; init; }
    [JsonPropertyName("custom_transparency")] public bool CustomTransparency { get; init; }
    [JsonPropertyName("loud_sound_reduction")] public bool LoudSoundReduction { get; init; }
    [JsonPropertyName("head_tracking")] public bool HeadTracking { get; init; }
    [JsonPropertyName("high_quality_mic")] public bool HighQualityMic { get; init; }
    [JsonPropertyName("custom_eq")] public bool CustomEq { get; init; }
    public int[] Controls { get; init; } = [];
    public bool Known => Models.Length > 0;
}

internal static class DeviceModels
{
    private static readonly DeviceModel Unknown = new();
    private static readonly Dictionary<string, DeviceModel> Models = Load();

    private static Dictionary<string, DeviceModel> Load()
    {
        using var stream = typeof(DeviceModels).Assembly.GetManifestResourceStream("ntpods.device-models.json")
            ?? throw new InvalidOperationException("Missing device model catalog.");
        var profiles = JsonSerializer.Deserialize<DeviceModel[]>(stream,
            new JsonSerializerOptions { PropertyNameCaseInsensitive = true })!;
        return profiles.SelectMany(profile => profile.Models.Select(model => (model, profile)))
            .ToDictionary(entry => entry.model, entry => entry.profile, StringComparer.OrdinalIgnoreCase);
    }

    public static DeviceModel Find(string? model) => Models.GetValueOrDefault(model?.Trim() ?? "", Unknown);
    public static bool Listening(Snapshot s) => Find(s.Model) is { Known: true } profile ? profile.Listening :
        s.DeviceControls.TryGetValue(0x0D, out var mode) && mode is >= 1 and <= 3;
    public static bool Adaptive(Snapshot s) => Find(s.Model).Adaptive && FirmwareMajor(s.Firmware) >= 6;
    public static bool AllowOffOption(Snapshot s) => Find(s.Model) is { AllowOffOption: true } profile &&
        (profile.Family == "airpods_pro_3" || FirmwareMajor(s.Firmware) >= 7 || s.DeviceControls.ContainsKey(Feature.AllowOff));
    public static bool SupportsControl(Snapshot s, byte id) => Find(s.Model) is { Known: true } profile
        ? profile.Controls.Contains(id) && (id != Feature.AllowOff || AllowOffOption(s)) &&
            (id != 0x35 || FirmwareMajor(s.Firmware) >= 8)
        : id is 0x01 or 0x0A or 0x16 or 0x17 or 0x18 or 0x1A or 0x1B or 0x1F or 0x23 or 0x24 or 0x31 or 0x3B or 0x40 &&
            s.DeviceControls.ContainsKey(id);

    public static bool OffAvailable(Snapshot s)
    {
        if (!Listening(s)) return false;
        var profile = Find(s.Model);
        if (profile.Known && !profile.AllowOffOption)
            return s.DeviceControls.GetValueOrDefault(Feature.AllowOff) != 2;
        if (s.AllowOffRequested is bool requested) return requested;
        if (s.DeviceControls.TryGetValue(Feature.AllowOff, out var value) && value is 1 or 2) return value == 1;
        if (profile.AllowOffOption && FirmwareMajor(s.Firmware) is > 0 and <= 6) return true;
        return s.DeviceControls.GetValueOrDefault((byte)0x0D) == 1;
    }

    public static int FirmwareMajor(string? firmware)
    {
        string value = (firmware ?? "").Trim();
        int end = 0;
        while (end < value.Length && value[end] is >= '0' and <= '9') end++;
        if (end == 0 || end == value.Length || !int.TryParse(value[..end], out int major) || major is < 1 or > 99)
            return 0;
        if (value[end] == '.')
        {
            var parts = value.Split('.');
            return parts.Length == 3 && parts.Skip(1).All(part => part.Length > 0 &&
                part.All(c => c is >= '0' and <= '9') && byte.TryParse(part, out _)) ? major : 0;
        }
        if (value[end++] is not (>= 'A' and <= 'Z')) return 0;
        int buildStart = end;
        while (end < value.Length && value[end] is >= '0' and <= '9') end++;
        if (end == buildStart) return 0;
        return end == value.Length || end == value.Length - 1 && value[end] is >= 'a' and <= 'z' ? major : 0;
    }

    public static bool HighQualityMic(Snapshot s) => Find(s.Model).HighQualityMic && FirmwareMajor(s.Firmware) >= 8;
    public static bool CustomEq(Snapshot s) => Find(s.Model).CustomEq && FirmwareMajor(s.Firmware) >= 9;
    public static bool HeadTracking(Snapshot s) => Find(s.Model).HeadTracking && FirmwareMajor(s.Firmware) > 0;
    public static bool CanChangeLoudSoundReduction(Snapshot s) => Find(s.Model).LoudSoundReduction &&
        s.Connected && !s.LinkStale && s.LoudSoundReduction.HasValue && !s.LoudSoundReductionPending;
    public static bool ShouldReadLoudSoundReduction(Snapshot s, bool alreadyRequested, bool force) =>
        Find(s.Model).LoudSoundReduction && s.Connected && !s.LinkStale && !s.LoudSoundReductionPending &&
        (force || !alreadyRequested && !s.LoudSoundReduction.HasValue);
    public static string DisplayName(Snapshot s, string fallback) => !string.IsNullOrWhiteSpace(s.DevName) ? s.DevName :
        Find(s.Model) is { Name.Length: > 0 } profile ? profile.Name : fallback;
    public static bool Headphone(Snapshot s) => Find(s.Model).Headphone ||
        !Find(s.Model).Known && BatteryPresentation.Present(s.Battery.Headphone).HasValue &&
        !BatteryPresentation.Present(s.Battery.Left).HasValue && !BatteryPresentation.Present(s.Battery.Right).HasValue;
}
