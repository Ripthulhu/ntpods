using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Services;

internal sealed class NearbyFinderPresentation
{
    private Snapshot _snapshot = new();
    private long _receivedAt;
    private long _startDeadline;
    public string? Target { get; private set; }
    public bool Starting { get; private set; }

    public bool CanStart(bool daemonReady) => daemonReady && Target is null &&
        !string.IsNullOrEmpty(_snapshot.DeviceId) && _snapshot.NearbyFinder is { AvailableIdentity: true, Active: false };

    public void Started(long now)
    {
        Target = _snapshot.DeviceId;
        Starting = true;
        _startDeadline = now + 5_000;
    }

    public string? Update(Snapshot snapshot, long now)
    {
        // Finder snapshots can arrive out of order within one daemon session.
        if (string.Equals(_snapshot.DeviceId, snapshot.DeviceId, StringComparison.OrdinalIgnoreCase) &&
            snapshot.NearbyFinder is { } incoming && _snapshot.NearbyFinder is { } current &&
            incoming.Revision < current.Revision) return null;
        string? stop = Target is not null && !string.Equals(Target, snapshot.DeviceId, StringComparison.OrdinalIgnoreCase)
            ? Stop() : null;
        _snapshot = snapshot;
        _receivedAt = now;
        if (snapshot.NearbyFinder?.Active == true) Starting = false;
        else if (!Starting) Stop();
        return stop;
    }

    public void Reset()
    {
        Stop();
        _snapshot = new();
        _receivedAt = 0;
    }

    public string? Stop()
    {
        string? target = Target;
        Target = null;
        Starting = false;
        return target;
    }

    public string? ExpireStart(long now) => Starting && now >= _startDeadline ? Stop() : null;

    public ulong? Age(long now)
    {
        if (_snapshot.NearbyFinder?.LastHeardAgeMs is not { } age) return null;
        ulong elapsed = (ulong)Math.Max(0, now - _receivedAt);
        return ulong.MaxValue - age < elapsed ? ulong.MaxValue : age + elapsed;
    }

    public short? Rssi => _snapshot.NearbyFinder?.Rssi is >= -126 and <= 20 ? _snapshot.NearbyFinder.Rssi : null;
    public double Signal => Rssi is { } rssi ? Math.Clamp((rssi + 100) / 60.0, 0, 1) * 100 : 0;
    // Ten seconds limits live guidance; quiet closed cases can advertise much less often.
    public bool Fresh(long now) => Target is not null && _snapshot.NearbyFinder is { Active: true, Paused: false }
        && Rssi is not null && Age(now) is <= 10_000;

    public string StatusKey(bool daemonReady, long now)
    {
        if (!daemonReady || _snapshot.NearbyFinder is null) return "NearbyFinder.Unavailable";
        if (!_snapshot.NearbyFinder.AvailableIdentity) return "NearbyFinder.NoIdentity";
        if (Starting) return "NearbyFinder.Starting";
        if (Target is null) return _snapshot.NearbyFinder.Active ? "NearbyFinder.Busy" : "NearbyFinder.Ready";
        if (_snapshot.NearbyFinder.Paused) return "NearbyFinder.Paused";
        if (!Fresh(now)) return Age(now).HasValue ? "NearbyFinder.NoRecentSignal" : "NearbyFinder.Waiting";
        return Rssi switch
        {
            >= -60 => "NearbyFinder.Strong",
            >= -80 => "NearbyFinder.Moderate",
            _ => "NearbyFinder.Weak",
        };
    }

    public string TrendKey(long now) => Fresh(now) ? _snapshot.NearbyFinder?.Trend switch
    {
        "stronger" => "NearbyFinder.Stronger",
        "weaker" => "NearbyFinder.Weaker",
        "steady" => "NearbyFinder.Steady",
        _ => "",
    } : "";

    public string LastHeardText(long now) => Age(now) switch
    {
        null => "",
        < 1_000 => Localize.Get("NearbyFinder.JustHeard"),
        < 60_000 and { } age => Localize.Get("NearbyFinder.SecondsAgo", age / 1_000),
        < 3_600_000 and { } age => Localize.Get("NearbyFinder.MinutesAgo", age / 60_000),
        { } age => Localize.Get("NearbyFinder.HoursAgo", age / 3_600_000),
    };
}
