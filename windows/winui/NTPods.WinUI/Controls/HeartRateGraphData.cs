namespace NTPods.WinUI.Controls;

internal sealed class HeartRateGraphData
{
    private (DateTime At, ushort Bpm)[] _samples = [];
    private int _buckets;
    private List<(int Index, ushort Bpm)> _points = [];
    public IReadOnlyList<(DateTime At, ushort Bpm)> Samples => _samples;
    public int Minimum { get; private set; }
    public int Maximum { get; private set; }
    public int Average { get; private set; }

    public bool SetSamples(IReadOnlyList<(DateTime At, ushort Bpm)> samples)
    {
        if (_samples.SequenceEqual(samples)) return false;
        _samples = samples.ToArray();
        _buckets = 0;
        Minimum = _samples.Length == 0 ? 0 : _samples.Min(sample => (int)sample.Bpm);
        Maximum = _samples.Length == 0 ? 0 : _samples.Max(sample => (int)sample.Bpm);
        Average = _samples.Length == 0 ? 0 : (int)Math.Round(_samples.Average(sample => (int)sample.Bpm));
        return true;
    }

    public IReadOnlyList<(int Index, ushort Bpm)> Points(int buckets)
    {
        buckets = Math.Max(2, buckets);
        if (_buckets == buckets) return _points;
        _buckets = buckets;
        int count = Math.Min(buckets, _samples.Length);
        _points = new(count);
        for (int bucket = 0; bucket < count; bucket++)
        {
            int from = bucket * _samples.Length / count, to = (bucket + 1) * _samples.Length / count;
            long total = 0;
            for (int i = from; i < to; i++) total += _samples[i].Bpm;
            _points.Add(((from + to - 1) / 2, (ushort)Math.Round(total / (double)(to - from))));
        }
        return _points;
    }

    public int Nearest(double position, bool live, DateTime now)
    {
        if (_samples.Length == 0) return -1;
        position = Math.Clamp(position, 0, 1);
        if (!live) return (int)Math.Round(position * (_samples.Length - 1));
        var at = now.AddSeconds((position - 1) * 60);
        int low = 0, high = _samples.Length - 1;
        while (low < high)
        {
            int mid = (low + high) / 2;
            if (_samples[mid].At < at) low = mid + 1;
            else high = mid;
        }
        return low > 0 && at - _samples[low - 1].At < _samples[low].At - at ? low - 1 : low;
    }
}
