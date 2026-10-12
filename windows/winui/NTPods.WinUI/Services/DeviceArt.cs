namespace NTPods.WinUI.Services;

internal sealed class DeviceArtworkState
{
    private string _deviceId = "";
    private string _model = "";

    public string Update(NTPods.WinUI.Ipc.Snapshot snapshot, string savedModel, string savedDeviceId)
    {
        if (!string.IsNullOrEmpty(snapshot.DeviceId) && snapshot.DeviceId != _deviceId)
        {
            _deviceId = snapshot.DeviceId;
            _model = "";
        }
        string saved = _deviceId.Length > 0 && _deviceId == savedDeviceId ? savedModel : "";
        return _model = DeviceArt.ResolveModel(snapshot.Model, _model, saved);
    }
}

/// Selects bundled artwork, reusing the closest shape when an exact image is missing.
/// This presentation choice never changes model identity or capability checks.
public static class DeviceArt
{
    private const string Base = "ms-appx:///Assets/";

    // BLE battery reports do not carry the model metadata from the audio link.
    public static string ResolveModel(string? reported, string? current, string? saved) =>
        Normalize(!string.IsNullOrWhiteSpace(reported) ? reported : !string.IsNullOrWhiteSpace(current) ? current : saved);

    public static string Family(string? model, bool headphone = false)
    {
        string family = DeviceModels.Find(model).Family;
        return family switch
        {
            "airpods_5" => "airpods_4",
            "powerbeats_pro_2" => "airpods_pro_2",
            "airpods_max" => family,
            _ when HasParts(family) => family,
            _ => headphone ? "airpods_max" : "airpods_pro_2"
        };
    }

    /// Whether this family ships the _left / _right / _case part images.
    private static bool HasParts(string family) => family is "airpods_1" or "airpods_2" or "airpods_3" or "airpods_4" or
        "airpods_pro_1" or "airpods_pro_2" or "airpods_pro_3";

    public static string MainImage(string? model, bool headphone = false) => $"{Base}{Family(model, headphone)}.png";

    public static string IslandImage(string? model, bool headphone = false)
    {
        string family = Family(model, headphone);
        return family.StartsWith("airpods_pro_", StringComparison.Ordinal) && HasParts(family) ? Base + "island_airpods_pro_buds.png" :
            HasParts(family) ? $"{Base}{family}_buds.png" : MainImage(model, headphone);
    }

    /// The left-bud image, or null when the family has no parts.
    public static string? LeftImage(string? model) => Part(model, "left");

    /// The right-bud image, or null when the family has no parts.
    public static string? RightImage(string? model) => Part(model, "right");

    /// The case image, or null when the family has no parts.
    public static string? CaseImage(string? model) => Part(model, "case");

    private static string? Part(string? model, string part)
    {
        var family = Family(model);
        return HasParts(family) ? $"{Base}{family}_{part}.png" : null;
    }

    private static string Normalize(string? model) =>
        string.IsNullOrWhiteSpace(model) ? "" : model.Trim().ToUpperInvariant();
}
