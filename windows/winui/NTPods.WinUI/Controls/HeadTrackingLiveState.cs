using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class HeadTrackingLiveState
{
    public const int Capacity = 100;
    private readonly List<DaemonEvent.HeadTracking> _samples = new(Capacity);
    private string _deviceId = "";
    public IReadOnlyList<DaemonEvent.HeadTracking> Samples => _samples;
    public bool Enabled { get; private set; }
    public bool Connected { get; private set; }
    public string State { get; private set; } = "idle";
    public string? Error { get; private set; }

    public void Update(Snapshot snapshot)
    {
        bool enabled = snapshot.HeadTrackingEnabled;
        Connected = snapshot.Connected && !snapshot.LinkStale;
        if (_deviceId != snapshot.DeviceId || !Connected || enabled && !Enabled) _samples.Clear();
        _deviceId = snapshot.DeviceId;
        Enabled = enabled;
        State = snapshot.HeadTrackingState;
        Error = snapshot.HeadTrackingError;
    }

    public bool Add(DaemonEvent.HeadTracking sample, ulong now)
    {
        if (!Connected || !Enabled || sample.TimestampMs > now || now - sample.TimestampMs >= 2_000 ||
            _samples.Count > 0 && sample.TimestampMs <= _samples[^1].TimestampMs) return false;
        if (_samples.Count == Capacity) _samples.RemoveAt(0);
        _samples.Add(sample);
        return true;
    }

    public bool IsFresh(ulong now) => Connected && Enabled && _samples.Count > 0 &&
        now >= _samples[^1].TimestampMs && now - _samples[^1].TimestampMs < 2_000;

    // Upstream uses a 100-sample window and leaves 20% of headroom above its peak.
    public double Range => Math.Max(1_000, _samples.Count == 0 ? 0 :
        _samples.Max(sample => Math.Max(Math.Abs((int)sample.Horizontal), Math.Abs((int)sample.Vertical))) * 1.2);
}
