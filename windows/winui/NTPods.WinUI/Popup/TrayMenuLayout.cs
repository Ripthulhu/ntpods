namespace NTPods.WinUI.Popup;

internal static class TrayMenuLayout
{
    public static (int X, int Y, int Width, int Height) Fit(
        int anchorX, int anchorY, int width, int height, int workX, int workY, int workWidth, int workHeight)
    {
        width = Math.Clamp(width, 1, Math.Max(1, workWidth));
        height = Math.Clamp(height, 1, Math.Max(1, workHeight));
        return (Math.Clamp(anchorX, workX, workX + Math.Max(0, workWidth - width)),
            Math.Clamp(anchorY - height, workY, workY + Math.Max(0, workHeight - height)), width, height);
    }
}
