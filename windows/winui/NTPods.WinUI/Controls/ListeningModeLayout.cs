using System;

namespace NTPods.WinUI.Controls;

/// Geometry of LibrePods' Apple selector: Adaptive grows from one segment to three.
internal readonly record struct ListeningModeLayout(double Width, double Segment, double OffWidth, double AdaptiveWidth, bool HasAdaptive = true)
{
    public double AdaptiveStart => OffWidth + Segment;
    public double AncStart => AdaptiveStart + AdaptiveWidth;

    public static ListeningModeLayout Create(double width, uint mode, bool allowOff, bool adaptive = true)
    {
        width = double.IsFinite(width) ? Math.Max(0, width) : 0;
        bool expanded = adaptive && mode == 4;
        bool showOff = allowOff && !expanded;
        double segment = width / (expanded ? 5 : 2 + (adaptive ? 1 : 0) + (showOff ? 1 : 0));
        return new(width, segment, showOff ? segment : 0, adaptive ? expanded ? 3 * segment : segment : 0, adaptive);
    }

    public (uint Mode, uint Strength, double Left) ResolveDrag(double left, uint strength, bool allowOff)
    {
        left = ClampLeft(left);
        uint mode = ModeAt(left);
        if (mode == 1 && !allowOff) mode = 3;
        return (mode, mode == 4 ? StrengthAt(left) : strength, left);
    }

    public double ClampLeft(double left) =>
        Math.Clamp(double.IsFinite(left) ? left : 0, 0, Math.Max(0, Width - Segment));

    public double LeftFor(uint mode, uint strength) => ClampLeft(mode switch
    {
        1 => 0,
        3 => OffWidth,
        4 => AdaptiveStart + Math.Min(strength, 100u) / 100d * Math.Max(0, AdaptiveWidth - Segment),
        2 => AncStart,
        _ => 0
    });

    public uint ModeAt(double left)
    {
        double center = ClampLeft(left) + Segment / 2;
        return OffWidth > 0 && center < OffWidth ? 1u :
            center < AdaptiveStart ? 3u :
            HasAdaptive && center < AncStart ? 4u : 2u;
    }

    public uint StrengthAt(double left)
    {
        double travel = AdaptiveWidth - Segment;
        return travel <= 0 ? 0 : (uint)Math.Clamp(Math.Round((ClampLeft(left) - AdaptiveStart) / travel * 100), 0, 100);
    }

    public double MarkerOpacity(double center, double left, double scaleX)
    {
        if (Segment <= 0 || !double.IsFinite(center) || !double.IsFinite(scaleX)) return 0;
        double thumbCenter = ClampLeft(left) + Segment / 2;
        // Compression shrinks the drawing, but the lens still samples its unscaled source.
        double halfWidth = Segment * Math.Clamp(scaleX, 1, 1.8) / 2;
        double clearance = Math.Abs(center - thumbCenter) - halfWidth - 6.5;
        double progress = Math.Clamp(clearance / 12, 0, 1);
        return progress * progress * (3 - 2 * progress);
    }
}
