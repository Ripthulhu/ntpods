using System.Globalization;
using System.Numerics;

namespace NTPods.WinUI.Controls;

internal readonly record struct SpeakerPosition(int Azimuth, int Elevation, int Gain);

internal sealed class SpeakerSelection
{
    internal int Mask { get; private set; } = 1;
    internal int Active { get; private set; } = 0;
    private static int First(int mask) => mask == 0 ? -1 : BitOperations.TrailingZeroCount((uint)mask);
    internal int PositionAnchor => Active == 3 ? First(Mask & ~(1 << 3)) : Active;
    internal bool Contains(int bit) => bit is >= 0 and <= 10 && (Mask & (1 << bit)) != 0;

    internal void Select(int bit, bool toggle = false)
    {
        if (bit is < 0 or > 10 || (0x63f & (1 << bit)) == 0) return;
        Mask = toggle ? Mask ^ (1 << bit) : 1 << bit;
        if (Contains(bit)) Active = bit;
        else if (Active == bit) Active = First(Mask);
    }

    internal void ForDrag(int bit)
    {
        if (Contains(bit)) Active = bit;
        else Select(bit);
    }

    internal void Reconcile(int availableMask)
    {
        if (Mask == 0) return;
        Mask &= availableMask;
        if (Mask == 0 && availableMask != 0) Mask = 1 << First(availableMask);
        if (!Contains(Active)) Active = First(Mask);
    }
}

internal sealed class SpatialRoomLayout(int mask)
{
    internal static readonly (int Bit, string Label)[] Channels =
        [(0, "FL"), (2, "C"), (1, "FR"), (9, "SL"), (10, "SR"), (4, "BL"), (5, "BR"), (3, "LFE")];
    private readonly SortedDictionary<int, SpeakerPosition> _overrides = new();
    internal int Mask { get; } = mask;
    internal int Width { get; set; } = 100;
    internal static bool Supports(int mask) => mask is 0x3f or 0x60f or 0x63f;
    internal bool Includes(int bit) => bit is >= 0 and <= 10 && (Mask & (1 << bit)) != 0;
    private SpeakerPosition Default(int bit) => new(bit switch
    {
        0 => -30, 1 => 30, 4 => Mask == 0x63f ? -150 : -110, 5 => Mask == 0x63f ? 150 : 110,
        9 => Mask == 0x63f ? -90 : -110, 10 => Mask == 0x63f ? 90 : 110, _ => 0
    }, 0, 0);
    internal SpeakerPosition Get(int bit) => _overrides.GetValueOrDefault(bit, Default(bit));
    internal void Set(int bit, SpeakerPosition value)
    {
        if (value == Default(bit)) _overrides.Remove(bit);
        else _overrides[bit] = value;
    }
    // Above 300% the front pair can reach the sides while the surround bed stays in place.
    private double FrontSpread(int bit) => Math.Max(Width - 300, 0) * 0.15 * (bit switch { 0 => -1, 1 => 1, _ => 0 });
    internal double Bearing(int bit)
    {
        double radians = Get(bit).Azimuth * Math.PI / 180;
        return Math.IEEERemainder(Math.Atan2(Math.Sin(radians) * Math.Min(Width, 300) / 100, Math.Cos(radians))
            + FrontSpread(bit) * Math.PI / 180, 2 * Math.PI);
    }
    internal int AzimuthAt(int bit, double bearing)
    {
        double radians = bearing - FrontSpread(bit) * Math.PI / 180;
        return (int)Math.Round(Math.Atan2(Math.Sin(radians) * 100 / Math.Min(Width, 300), Math.Cos(radians)) * 180 / Math.PI);
    }

    private SpatialRoomLayout? _dragStart;
    internal void BeginDrag()
    {
        // Use the original bearings for the whole gesture so rounding to stored degrees cannot accumulate.
        _dragStart = new(Mask) { Width = Width };
        foreach (var (bit, position) in _overrides) _dragStart.Set(bit, position);
    }

    internal bool DragSelection(int mask, int anchor, double bearing, int elevation)
    {
        var start = _dragStart;
        if (start == null || !double.IsFinite(bearing)) return false;
        double delta = bearing * Math.PI / 180 - start.Bearing(anchor);
        return Move(mask, anchor, elevation, start, bit => start.AzimuthAt(bit, start.Bearing(bit) + delta));
    }

    internal bool MoveSelection(int mask, int anchor, int azimuth, int elevation)
    {
        int delta = azimuth - Get(anchor).Azimuth;
        return Move(mask, anchor, elevation, this, bit => bit == anchor ? azimuth
            : (int)Math.IEEERemainder(Get(bit).Azimuth + delta, 360));
    }

    private bool Move(int mask, int anchor, int elevation, SpatialRoomLayout origin, Func<int, int> azimuth)
    {
        mask &= Mask & ~(1 << 3);
        if (!Includes(anchor) || (mask & (1 << anchor)) == 0) return false;
        int up = elevation - origin.Get(anchor).Elevation;
        foreach (var (bit, _) in Channels)
            if ((mask & (1 << bit)) != 0)
                up = Math.Clamp(up, -90 - origin.Get(bit).Elevation, 90 - origin.Get(bit).Elevation);
        bool changed = false;
        foreach (var (bit, _) in Channels)
        {
            if ((mask & (1 << bit)) == 0) continue;
            var before = Get(bit);
            var after = before with { Azimuth = azimuth(bit), Elevation = origin.Get(bit).Elevation + up };
            if (before == after) continue;
            Set(bit, after);
            changed = true;
        }
        return changed;
    }

    internal bool AdjustGain(int mask, int anchor, int gain)
    {
        mask &= Mask;
        if (!Includes(anchor) || (mask & (1 << anchor)) == 0) return false;
        int delta = gain - Get(anchor).Gain;
        foreach (var (bit, _) in Channels)
            if ((mask & (1 << bit)) != 0)
                delta = Math.Clamp(delta, -24 - Get(bit).Gain, -Get(bit).Gain);
        if (delta == 0) return false;
        foreach (var (bit, _) in Channels)
            if ((mask & (1 << bit)) != 0) Set(bit, Get(bit) with { Gain = Get(bit).Gain + delta });
        return true;
    }
    internal string Command() => string.Join(" ", new[] { FormattableString.Invariant($"ROOM {Width}") }
        .Concat(_overrides.Select(p => FormattableString.Invariant($"{p.Key},{p.Value.Azimuth},{p.Value.Elevation},{p.Value.Gain}"))));
    internal static SpatialRoomLayout Load(int mask, string? command)
    {
        var room = new SpatialRoomLayout(mask);
        if (command is null || command.Length > 256) return room;
        var parts = command.Split(' ');
        if (parts.Length < 2 || parts[0] != "ROOM" || !Int(parts[1], out var width) || width is < 50 or > 500) return room;
        var seen = new HashSet<int>();
        foreach (var field in parts.Skip(2))
        {
            var v = field.Split(',');
            if (v.Length != 4 || !Int(v[0], out var bit) || bit is < 0 or > 10 || !room.Includes(bit) || !seen.Add(bit)
                || !Int(v[1], out var az) || az is < -180 or > 180 || !Int(v[2], out var el) || el is < -90 or > 90
                || !Int(v[3], out var gain) || gain is < -24 or > 0 || (bit == 3 && (az != 0 || el != 0)))
                return new(mask);
            room.Set(bit, new(az, el, gain));
        }
        room.Width = width;
        return room;
    }
    internal static bool Int(string s, out int n) => int.TryParse(s, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out n);
}

internal sealed record RoomFrame(int? Yaw, int? Pitch, int? Roll, int Mask, int Active, ulong? Peaks = null)
{
    // Store each channel's 0..60 dB above the meter floor in mask order.
    internal int Level(int bit)
    {
        if (Peaks is not ulong peaks) return (Active & (1 << bit)) == 0 ? -60 : -18;
        int index = BitOperations.PopCount((uint)(Mask & ((1 << bit) - 1)));
        return (int)((peaks >> (6 * index)) & 63) - 60;
    }

    internal static RoomFrame? Parse(string line)
    {
        if (line.Length > 120) return null;
        var p = line.Split(' ');
        if (p.Length is not (6 or 7) || p[0] != "ROOM_POSE" || !SpatialRoomLayout.Int(p[4], out var mask) || !SpatialRoomLayout.Supports(mask)
            || !SpatialRoomLayout.Int(p[5], out var active) || (active & ~mask) != 0) return null;
        int? Angle(string s, int limit) => SpatialRoomLayout.Int(s, out var v) && Math.Abs((long)v) <= limit ? v : null;
        var yaw = Angle(p[1], 180);
        var pitch = Angle(p[2], 90);
        var roll = Angle(p[3], 180);
        if ((yaw == null && p[1] != "none") || (pitch == null && p[2] != "none") || (roll == null && p[3] != "none")
            || pitch.HasValue != roll.HasValue || (pitch.HasValue && !yaw.HasValue)) return null;
        ulong? peaks = null;
        if (p.Length == 7)
        {
            var levels = p[6].Split(',');
            if (levels.Length != BitOperations.PopCount((uint)mask)) return null;
            ulong packed = 0;
            for (int i = 0; i < levels.Length; i++)
            {
                if (!SpatialRoomLayout.Int(levels[i], out int db) || db is < -60 or > 0) return null;
                packed |= (ulong)(db + 60) << (6 * i);
            }
            peaks = packed;
        }
        return new(yaw, pitch, roll, mask, active, peaks);
    }
}
