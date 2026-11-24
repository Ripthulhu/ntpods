using System.Drawing;
using NTPods.WinUI.Controls;

internal static class SpeakerMapLabelChecks
{
    internal static void Run(Action<bool, string> check)
    {
        var viewport = new RectangleF(0, 0, 272, 218);
        var head = new RectangleF(114, 85, 44, 48);
        static RectangleF Speaker(float x, float y) => new(x - 23, y - 18, 46, 36);
        var regular = new[] { Speaker(136, 49), Speaker(82, 57), Speaker(190, 57) };
        check(SpeakerMapLabels.Place(viewport, head, regular).SequenceEqual(regular),
            "Separated labels must stay at their actual speaker coordinates.");
        var narrow = new[] { Speaker(136, 49), Speaker(106, 51), Speaker(166, 51) };
        var coincident = Enumerable.Repeat(Speaker(136, 35), 7).ToArray();
        foreach (var targets in new[] { narrow, coincident })
        {
            var labels = SpeakerMapLabels.Place(viewport, head, targets);
            check(labels.SequenceEqual(SpeakerMapLabels.Place(viewport, head, targets)),
                "Equal room geometry must produce stable label placement.");
            for (int i = 0; i < labels.Length; i++)
            {
                check(viewport.Contains(labels[i]) && !head.IntersectsWith(labels[i]) &&
                      labels.Take(i).All(other => !labels[i].IntersectsWith(other)),
                    "Every close speaker must remain independently visible inside the room.");
                if (labels[i] != targets[i])
                    check(!labels[i].Contains(targets[i].Left + targets[i].Width / 2, targets[i].Top + targets[i].Height / 2),
                        "A moved label must expose its actual speaker point for the leader.");
            }
        }
        Console.WriteLine("Speaker label placement checks passed.");
    }
}
