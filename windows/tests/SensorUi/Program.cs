using NTPods.WinUI.Controls;

static void Check(bool condition, string message)
{
    if (!condition) throw new InvalidOperationException(message);
}

var data = new HeartRateGraphData();
var now = new DateTime(2026, 1, 1, 12, 0, 0);
Check(data.Nearest(.5, false, now) == -1 && data.Points(0).Count == 0, "Empty charts have no selected reading.");
var raw = Enumerable.Range(0, 10_000).Select(i => (now.AddSeconds(i), (ushort)(60 + i % 40))).ToArray();
raw[1234] = (raw[1234].Item1, 180);
Check(data.SetSamples(raw), "New history must refresh the cached chart data.");
Check(data.Minimum == 60 && data.Maximum == 180 && data.Average == (int)Math.Round(raw.Average(point => point.Item2)),
    "Statistics must retain raw readings that disappear into graph buckets.");
var points = data.Points(100);
Check(points.Count == 100 && ReferenceEquals(points, data.Points(100)), "Pointer inspection must reuse downsampled points.");
Check(!data.SetSamples(raw) && ReferenceEquals(points, data.Points(100)), "Identical snapshots must not invalidate the graph cache.");
int selected = data.Nearest(1234d / 9999, false, now);
Check(selected == 1234 && data.Samples[selected].Bpm == 180, "Selection must return the exact sample, not a bucket average.");
Check(data.Nearest(-2, false, now) == 0 && data.Nearest(2, false, now) == 9999, "Selection must clamp to real endpoints.");
Check(data.Points(50).Count == 50 && !ReferenceEquals(points, data.Points(50)), "Resize must rebuild only the point buckets.");
data.SetSamples([(now.AddSeconds(-50), 65), (now.AddSeconds(-10), 75), (now.AddSeconds(-1), 85)]);
Check(data.Nearest(.5, true, now) == 1 && data.Nearest(.98, true, now) == 2 && data.Nearest(0, true, now) == 0,
    "Live selection must follow timestamps across missing samples.");
Check(data.Points(100).Count == 3 && data.Points(100)[1].Bpm == 75, "Small data sets must preserve every sample.");
data.SetSamples([]);
Check(data.Points(100).Count == 0 && data.Minimum == 0 && data.Nearest(1, false, now) == -1, "Clearing history must clear cached points and selection.");
Console.WriteLine("Heart-rate graph caching, exact readings and timestamp selection checks passed.");
