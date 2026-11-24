using System.Globalization;
using System.Numerics;

namespace NTPods.WinUI.Controls;

internal readonly record struct SpeakerPosition(int Azimuth, int Elevation, int Gain);

internal sealed class SpatialRoomLayout(int mask)
{
    internal static readonly (int Bit, string Label)[] Channels =
        [(0, "FL"), (2, "C"), (1, "FR"), (9, "SL"), (10, "SR"), (4, "BL"), (5, "BR"), (3, "LFE")];
    private readonly SortedDictionary<int, SpeakerPosition> _overrides = new();
    internal int Mask { get; } = mask;
    internal int Width { get; set; } = 100;
    internal static bool Supports(int mask) => mask is 0x3f or 0x60f or 0x63f;
    internal bool Includes(int bit) => (Mask & (1 << bit)) != 0;
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
    internal double Bearing(int bit)
    {
        double radians = Get(bit).Azimuth * Math.PI / 180;
        return Math.Atan2(Math.Sin(radians) * Width / 100, Math.Cos(radians));
    }
    internal int AzimuthAt(double right, double forward) =>
        (int)Math.Round(Math.Atan2(right * 100 / Width, forward) * 180 / Math.PI);
    internal string Command() => string.Join(" ", new[] { FormattableString.Invariant($"ROOM {Width}") }
        .Concat(_overrides.Select(p => FormattableString.Invariant($"{p.Key},{p.Value.Azimuth},{p.Value.Elevation},{p.Value.Gain}"))));
    internal static SpatialRoomLayout Load(int mask, string? command)
    {
        var room = new SpatialRoomLayout(mask);
        if (command is null || command.Length > 256) return room;
        var parts = command.Split(' ');
        if (parts.Length < 2 || parts[0] != "ROOM" || !Int(parts[1], out var width) || width is < 50 or > 300) return room;
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
