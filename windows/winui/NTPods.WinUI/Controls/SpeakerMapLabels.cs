using System.Drawing;

namespace NTPods.WinUI.Controls;

internal static class SpeakerMapLabels
{
    // A target is a button centered on its projected speaker. Solve this small
    // layout only when geometry changes. Telemetry never moves a label.
    internal static RectangleF[] Place(RectangleF viewport, RectangleF listener, RectangleF[] targets)
    {
        const float gap = 4;
        var occupied = new List<RectangleF> { listener };
        var result = new RectangleF[targets.Length];
        for (int i = 0; i < targets.Length; i++)
        {
            var target = targets[i];
            float x = target.X + target.Width / 2, y = target.Y + target.Height / 2;
            var xs = new List<float> { target.Left, x - target.Width - gap, x + gap };
            var ys = new List<float> { target.Top, y - target.Height - gap, y + gap };
            foreach (var block in occupied)
            {
                xs.Add(block.Left - target.Width - gap);
                xs.Add(block.Right + gap);
                ys.Add(block.Top - target.Height - gap);
                ys.Add(block.Bottom + gap);
            }

            var best = (overlap: float.PositiveInfinity, covered: int.MaxValue, travel: float.PositiveInfinity);
            foreach (float proposedX in xs)
            foreach (float proposedY in ys)
            {
                var box = new RectangleF(
                    Math.Clamp(proposedX, viewport.Left, Math.Max(viewport.Left, viewport.Right - target.Width)),
                    Math.Clamp(proposedY, viewport.Top, Math.Max(viewport.Top, viewport.Bottom - target.Height)),
                    target.Width, target.Height);
                float dx = box.Left - target.Left, dy = box.Top - target.Top;
                float travel = dx * dx + dy * dy;
                float overlap = 0;
                foreach (var block in occupied)
                {
                    var intersection = RectangleF.Intersect(box, RectangleF.Inflate(block, gap, gap));
                    overlap += intersection.Width * intersection.Height;
                }
                int covered = 0;
                var padded = RectangleF.Inflate(box, 2, 2);
                for (int j = 0; j < targets.Length; j++)
                {
                    if (j == i && travel < 1) continue;
                    var other = targets[j];
                    if (padded.Contains(other.X + other.Width / 2, other.Y + other.Height / 2)) covered++;
                }
                var score = (overlap, covered, travel);
                if (score.CompareTo(best) >= 0) continue;
                best = score;
                result[i] = box;
            }
            occupied.Add(result[i]);
        }
        return result;
    }
}
