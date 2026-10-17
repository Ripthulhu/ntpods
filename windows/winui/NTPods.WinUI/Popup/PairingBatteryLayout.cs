namespace NTPods.WinUI.Popup;

internal static class PairingBatteryLayout
{
    public static bool Split(byte? left, byte? right, bool leftCharging, bool rightCharging) =>
        left != right || left.HasValue && right.HasValue && leftCharging != rightCharging;
}
