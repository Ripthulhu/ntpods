using System.Numerics;
using NTPods.WinUI.Controls;

internal static class LensChecks
{
    private static void Check(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }

    internal static void Run()
    {
        var sum = ListeningModeLens.Weights.Aggregate(Vector4.Zero, (total, value) => total + value);
        Check(Vector4.Distance(sum, Vector4.One) < .000001, "Seven spectral samples must preserve a flat RGBA source");
        foreach (float width in new[] { 54f, 72, 120, 213.333f })
        {
            Check(ListeningModeLens.Sample(width, new(width / 2, 27)).Shift == Vector2.Zero, "The deep interior must not refract");
            var top = ListeningModeLens.Sample(width, new(width / 2, 0));
            Check(Vector2.Distance(top.Shift, new(0, 36)) < .0001, "The negative upstream refraction amount must bend the top edge inward");
            var left = ListeningModeLens.Sample(width, new(0, 27));
            Check(Vector2.Distance(left.Shift, new(36, 0)) < .0001, "The negative upstream refraction amount must bend the left edge inward");
            var halfDepth = ListeningModeLens.Sample(width, new(width / 2, 6));
            Check(Vector2.Distance(halfDepth.Shift, new(0, 4.8230853f)) < .0001,
                "The 6-DIP depth must equal upstream circleMap(0.5) times -36");
            Check(ListeningModeLens.Sample(width, new(width / 2, 12)).Shift == Vector2.Zero, "Refraction must end at the 12-DIP boundary");
            foreach (float v in new[] { .001f, .1f, .25f, .5f, .75f, .9f, .999f })
            foreach (float u in new[] { .001f, .1f, .25f, .5f, .75f, .9f, .999f })
            {
                var value = ListeningModeLens.Sample(width, new(u * width, v * 54));
                var opposite = ListeningModeLens.Sample(width, new((1 - u) * width, (1 - v) * 54));
                Check(float.IsFinite(value.Shift.X) && float.IsFinite(value.Shift.Y), "Lens coordinates must remain finite at axes and corners");
                Check(Vector2.Distance(value.Shift, -opposite.Shift) < .003, "Opposite edges must bend symmetrically");
                Check(Math.Abs(value.Dispersion) <= 1 && value.Highlight >= 0 && value.Highlight <= 1.000001,
                    "Dispersion and directional rim must remain bounded");
                foreach (float spectralOffset in ListeningModeLens.Dispersion)
                {
                    var shift = value.Shift * (1 + spectralOffset * value.Dispersion);
                    Check(Math.Abs(shift.X) <= 72.0001 && Math.Abs(shift.Y) <= 72.0001, "The 144-DIP map must contain every spectral shift");
                }
            }
        }
        var raster = ListeningModeLens.Create(72, 144);
        Check(raster.PixelWidth == 108 && raster.PixelHeight == 81, "Lens rasters must respect display DPI");
        foreach (var map in raster.Maps)
        {
            Check(map.Length == 108 * 81 * 16, "Displacement maps must store four full-precision floats per pixel");
            foreach (int pixel in new[] { 0, 53, 107, 40 * 108 + 54, 80 * 108, 108 * 81 - 1 })
            {
                int offset = pixel * 16;
                float red = BitConverter.ToSingle(map, offset), green = BitConverter.ToSingle(map, offset + 4);
                Check(float.IsFinite(red) && red >= 0 && red <= 1 && float.IsFinite(green) && green >= 0 && green <= 1,
                    "Encoded displacement channels must be finite and normalized");
                Check(BitConverter.ToSingle(map, offset + 12) == 1, "Map alpha must not attenuate the displacement");
            }
        }
        foreach (float invalid in new[] { 0f, -1, float.NaN, float.PositiveInfinity, 2049 })
        {
            try { ListeningModeLens.Create(invalid, 96); throw new InvalidOperationException("Invalid geometry was accepted"); }
            catch (ArgumentOutOfRangeException) { }
        }
    }
}
