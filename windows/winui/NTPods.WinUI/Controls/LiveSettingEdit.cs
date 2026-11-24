using System.Collections.Generic;
using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

// One submitted value and one replaceable successor; reports never become user edits.
internal sealed class LiveSettingEdit<T> where T : class
{
    private string _owner = "";
    private T? _reported, _pending;
    private bool _pendingSeen;
    private long _requestedAt;
    private string? _error;
    public T? Desired { get; private set; }
    public T? Requested { get; private set; }
    public T? Displayed => Desired ?? Requested ?? _pending ?? _reported;
    public bool Busy => Requested is not null || _pending is not null;

    public void Update(string owner, bool available, T? reported, T? pending, string? error = null)
    {
        var previousReport = _reported;
        if (_owner != owner || !available) Cancel();
        _owner = owner;
        _reported = reported;
        _pending = available ? pending : null;
        if (!available) { _error = error; return; }
        if (error != _error && !string.IsNullOrEmpty(error)) { Cancel(); _pending = pending; }
        if (Requested is null && Desired is not null && (!Same(previousReport, reported) || pending is not null))
        { Cancel(); _pending = pending; }
        if (Requested is not null)
        {
            if (pending is not null && !Same(pending, Requested) ||
                !Same(previousReport, reported) && !Same(reported, Requested)) { Cancel(); _pending = pending; }
            else if (pending is not null) _pendingSeen = true;
            else if (_pendingSeen || Same(reported, Requested))
                EndRequest();
        }
        if (!Busy && Same(Desired, reported)) Desired = null;
        _error = error;
    }

    public void Edit(T? value) => Desired = value;

    public T? Take(long? now = null)
    {
        if (Busy || Desired is null) return null;
        var value = Desired;
        Desired = null;
        if (Same(value, _reported)) return null;
        Requested = value;
        _requestedAt = now ?? System.Environment.TickCount64;
        _pendingSeen = false;
        return value;
    }

    public bool Expire(long now, long timeoutMs)
    {
        if (Requested is null || now - _requestedAt < timeoutMs) return false;
        Cancel();
        return true;
    }

    public void EndRequest() { Requested = null; _pendingSeen = false; }
    public void Cancel() { Desired = null; EndRequest(); _pending = null; }
    private static bool Same(T? left, T? right) => EqualityComparer<T?>.Default.Equals(left, right);
}

internal sealed record LiveControlValue(uint Value);
internal sealed record LiveBitrateValue(string Mode, bool BestEffort, uint Kbps, bool Variable);

internal static class LiveDeviceChoices
{
    public static uint? Hold(int left, int right) => left is 0 or 1 && right is 0 or 1
        ? ((left == 0 ? 5u : 1u) << 8) | (right == 0 ? 5u : 1u) : null;
    public static int HoldIndex(uint? value, bool right) => value is uint raw
        ? ((raw >> (right ? 0 : 8)) & 0xFF) switch { 5 => 0, 1 => 1, _ => -1 } : -1;
    public static uint? NoiseCycle(uint mask) => mask <= 15 && System.Numerics.BitOperations.PopCount(mask) >= 2 ? mask : null;
}

internal static class LiveSettingScope
{
    public static string For(Snapshot snapshot, ulong clientEpoch = 0) =>
        $"{snapshot.DeviceId}/{clientEpoch}/{snapshot.HeadphoneAccommodations?.Session ?? 0}/" +
        $"{snapshot.Dongle.Session}/{snapshot.Dongle.Generation}/{snapshot.Dongle.PairToken}";
}
