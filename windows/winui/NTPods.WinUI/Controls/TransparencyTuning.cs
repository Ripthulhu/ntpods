using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal static class TransparencyTuning
{
    public static float Amplification(TransparencySettings value) => Math.Min(value.LeftAmplification, value.RightAmplification);
    public static float Balance(TransparencySettings value) => value.RightAmplification - value.LeftAmplification;
    public static bool CanEditCombined(TransparencySettings value) => value.IsValid && Amplification(value) is >= -1 and <= 1 &&
        Balance(value) is >= -1 and <= 1;

    public static TransparencySettings WithAmplification(TransparencySettings value, float amplification)
    {
        if (!CanEditCombined(value) || !float.IsFinite(amplification)) return value;
        return WithGains(value, Math.Clamp(amplification, -1, 1), Balance(value));
    }

    public static TransparencySettings WithBalance(TransparencySettings value, float balance) =>
        CanEditCombined(value) && float.IsFinite(balance) ? WithGains(value, Amplification(value), Math.Clamp(balance, -1, 1)) : value;

    // PersonalAudio v4: balance raises one ear above the shared base amplification.
    private static TransparencySettings WithGains(TransparencySettings value, float amplification, float balance) => value with
    {
        LeftAmplification = Math.Clamp(amplification - Math.Min(balance, 0), -1, 2),
        RightAmplification = Math.Clamp(amplification + Math.Max(balance, 0), -1, 2)
    };
}
