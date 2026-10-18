using NTPods.WinUI.Services;

namespace NTPods.WinUI.Popup;

internal static class PairingArtworkPolicy
{
    internal static bool SupportsLoop(string? model, bool headphone) => !headphone &&
        DeviceModels.Find(model).Family is "airpods_pro_1" or "airpods_pro_2" or "airpods_pro_3";

    internal static bool ShouldPlay(string? model, bool headphone, bool active, bool animations, bool highContrast, bool failed) =>
        active && animations && !highContrast && !failed && SupportsLoop(model, headphone);
}
