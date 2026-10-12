using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class HeartRateLiveState
{
    public const long FreshForMs = 10_000;
    private ulong? _latestTimestamp;
    private ushort? _latestBpm;
    private string _deviceId = "";
    public bool Enabled { get; private set; }
    public List<(DateTime At, ushort Bpm)> Samples { get; } = new();

    public void Update(Snapshot snapshot, DateTimeOffset now)
    {
        if (_deviceId != snapshot.DeviceId)
        {
            _latestTimestamp = null;
            _latestBpm = null;
            Samples.Clear();
            _deviceId = snapshot.DeviceId;
        }
        Enabled = snapshot.HeartRateEnabled;
        if (!snapshot.Connected || !Enabled)
        {
            _latestTimestamp = null;
            _latestBpm = null;
            Samples.Clear();
            return;
        }
        if (snapshot.HeartRate is ushort bpm && snapshot.HeartRateTimestampMs is ulong timestamp &&
            timestamp <= (ulong)now.ToUnixTimeMilliseconds() &&
            (ulong)now.ToUnixTimeMilliseconds() - timestamp <= FreshForMs)
        {
            if (_latestTimestamp is null || timestamp >= _latestTimestamp)
            {
                if (timestamp != _latestTimestamp)
                    Samples.Add((DateTimeOffset.FromUnixTimeMilliseconds((long)timestamp).LocalDateTime, bpm));
                _latestTimestamp = timestamp;
                _latestBpm = bpm;
            }
        }
        else _latestBpm = null;
        Expire(now);
    }

    public ushort? Current(DateTimeOffset now) => _latestTimestamp is ulong timestamp &&
        timestamp <= (ulong)now.ToUnixTimeMilliseconds() &&
        (ulong)now.ToUnixTimeMilliseconds() - timestamp <= FreshForMs ? _latestBpm : null;

    public void Expire(DateTimeOffset now)
    {
        var cutoff = now.LocalDateTime.AddMinutes(-1);
        Samples.RemoveAll(sample => sample.At < cutoff);
        if (Samples.Count > 120) Samples.RemoveRange(0, Samples.Count - 120);
    }
}
