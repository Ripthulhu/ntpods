namespace NTPods.WinUI.Services;

internal static class WindowSizing
{
    public static (int Width, int Height) Minimum(double scale, int frameWidth, int frameHeight,
        int workWidth, int workHeight)
    {
        if (!double.IsFinite(scale) || scale <= 0) scale = 1;
        return (Math.Min(workWidth, (int)Math.Ceiling(420 * scale) + Math.Max(0, frameWidth)),
            Math.Min(workHeight, (int)Math.Ceiling(540 * scale) + Math.Max(0, frameHeight)));
    }
}
