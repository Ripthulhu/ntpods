namespace NTPods.WinUI.Controls;

internal static class TransparencySliderMetrics
{
    internal const float Width = 40, Height = 24, CardInset = 24, SliderMargin = 10, PressedScale = 1.5f;
    internal static float Scale(double progress) => 1 + (PressedScale - 1) * (float)Math.Clamp(progress, 0, 1);
}
