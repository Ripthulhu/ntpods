// Rounded-rectangle lens math adapted from AndroidLiquidGlass 2.0.1.
// Copyright 2025 Kyant. Licensed under Apache-2.0; see licenses/AndroidLiquidGlass-Apache-2.0.txt.
using System.Numerics;

namespace NTPods.WinUI.Controls;

internal static class ListeningModeLens
{
    internal const float Height = 54;
    internal const float MapAmount = 144;
    internal static readonly float[] Dispersion = [1, 2f / 3, 1f / 3, 0, -1f / 3, -2f / 3, -1];
    internal static readonly Vector4[] Weights =
    [
        new(2f / 7, 0, 0, 1f / 7), new(2f / 7, 1f / 7, 0, 1f / 7),
        new(2f / 7, 2f / 7, 0, 1f / 7), new(0, 2f / 7, 0, 1f / 7),
        new(0, 2f / 7, 1f / 3, 1f / 7), new(0, 0, 1f / 3, 1f / 7),
        new(1f / 7, 0, 1f / 3, 1f / 7)
    ];

    internal static (Vector2 Shift, float Dispersion, float Highlight) Sample(float width, Vector2 point)
    {
        var half = new Vector2(width / 2, Height / 2);
        var centered = point - half;
        float radius = Math.Min(half.X, half.Y);
        var corner = Vector2.Abs(centered) - (half - new Vector2(radius));
        float distance = Vector2.Max(corner, Vector2.Zero).Length() - radius + Math.Min(Math.Max(corner.X, corner.Y), 0);
        var gradient = Gradient(centered, half, Math.Min(radius * 1.5f, Math.Min(half.X, half.Y)));
        float highlight = Math.Abs(Vector2.Dot(gradient, new Vector2(MathF.Sqrt(.5f))));
        if (-distance >= 12) return (Vector2.Zero, 0, highlight);
        float circle = 1 + Math.Min(distance, 0) / 12;
        float depth = (1 - MathF.Sqrt(Math.Max(0, 1 - circle * circle))) * -36;
        var direction = Normalize(gradient + Normalize(centered));
        return (depth * direction, centered.X * centered.Y / (half.X * half.Y), highlight);
    }

    private static Vector2 Gradient(Vector2 point, Vector2 half, float radius)
    {
        var corner = Vector2.Abs(point) - (half - new Vector2(radius));
        var signs = new Vector2(Math.Sign(point.X), Math.Sign(point.Y));
        if (corner.X >= 0 || corner.Y >= 0)
            return signs * Normalize(Vector2.Max(corner, Vector2.Zero));
        return signs * (corner.X >= corner.Y ? Vector2.UnitX : Vector2.UnitY);
    }

    private static Vector2 Normalize(Vector2 value) => value.LengthSquared() > 0 ? Vector2.Normalize(value) : Vector2.Zero;

    internal sealed record Raster(float Width, int PixelWidth, int PixelHeight, byte[][] Maps, byte[] Highlight);

    internal static Raster Create(float width, float dpi)
    {
        if (!float.IsFinite(width) || width <= 0 || width > 2048 || !float.IsFinite(dpi) || dpi < 48 || dpi > 768)
            throw new ArgumentOutOfRangeException(nameof(width));
        int pixelWidth = (int)Math.Ceiling(width * dpi / 96);
        int pixelHeight = (int)Math.Ceiling(Height * dpi / 96);
        var maps = Enumerable.Range(0, 7).Select(_ => new float[pixelWidth * pixelHeight * 4]).ToArray();
        var highlight = new byte[pixelWidth * pixelHeight * 4];
        for (int y = 0; y < pixelHeight; y++)
        for (int x = 0; x < pixelWidth; x++)
        {
            var sample = Sample(width, new((x + .5f) * width / pixelWidth, (y + .5f) * Height / pixelHeight));
            int offset = (y * pixelWidth + x) * 4;
            for (int channel = 0; channel < 7; channel++)
            {
                var shift = sample.Shift * (1 + Dispersion[channel] * sample.Dispersion);
                maps[channel][offset] = .5f + shift.X / MapAmount;
                maps[channel][offset + 1] = .5f + shift.Y / MapAmount;
                maps[channel][offset + 3] = 1;
            }
            byte light = (byte)Math.Clamp(Math.Round(sample.Highlight * 127.5f), 0, 255);
            for (int component = 0; component < 4; component++) highlight[offset + component] = light;
        }
        return new(width, pixelWidth, pixelHeight, maps.Select(values =>
        {
            var bytes = new byte[values.Length * sizeof(float)];
            Buffer.BlockCopy(values, 0, bytes, 0, bytes.Length);
            return bytes;
        }).ToArray(), highlight);
    }
}
