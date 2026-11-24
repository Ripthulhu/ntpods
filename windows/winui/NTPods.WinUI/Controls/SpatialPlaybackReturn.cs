using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

// The daemon's PairToken changes when the USB personality re-enumerates. PairedDeviceId
// identifies the headphones across that transition, while Serial identifies the physical Pico.
internal sealed class SpatialPlaybackReturn
{
    private string? _serial;
    private string? _pair;
    private ulong _session;
    private long? _exitedAt;
    internal bool Armed => _serial is not null;
    internal bool Waiting { get; private set; }

    internal void Arm(DongleState d)
    {
        Clear();
        if (d.Serial is not { Length: > 0 } || d.PairedDeviceId is not { Length: 12 } pair ||
            !pair.All(Uri.IsHexDigit) || !pair.Any(c => c != '0') ||
            pair.Equals("FFFFFFFFFFFF", StringComparison.OrdinalIgnoreCase) || d.Session == 0) return;
        _serial = d.Serial;
        _pair = pair;
        _session = d.Session;
    }

    internal void Clear()
    {
        _serial = _pair = null;
        _exitedAt = null;
        Waiting = false;
    }

    internal void Exited(long now) => _exitedAt = now;

    internal void Observe(DongleState d, long now)
    {
        if (!Armed) return;
        // WASAPI can report removal before the daemon observes USB. An unrelated
        // renderer failure must not arm recovery for a much later disconnect.
        if (!Waiting && _exitedAt is long end && now - end > 3000 ||
            d.Ambiguous || d.Serial is not null && d.Serial != _serial ||
            d.PairedDeviceId is not null && !string.Equals(d.PairedDeviceId, _pair, StringComparison.OrdinalIgnoreCase))
        {
            Clear();
            return;
        }
        if (!d.Present || !d.UsbAudioAvailable || !d.Connected || d.Session != _session ||
            !d.Selected || d.PairedDeviceId is null)
            Waiting = true;
    }

    internal bool Take(DongleState d, bool ready)
    {
        if (!Waiting || !ready || d.Serial != _serial ||
            !string.Equals(d.PairedDeviceId, _pair, StringComparison.OrdinalIgnoreCase)) return false;
        // Only SPATIAL_READY can rearm this after the one relaunch succeeds.
        Clear();
        return true;
    }
}

