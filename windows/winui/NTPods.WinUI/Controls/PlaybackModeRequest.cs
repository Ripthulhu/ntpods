using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class PlaybackModeRequest
{
    public string? Requested { get; private set; }
    public static string PreferredMode(Snapshot s) => s.PlaybackPreferredMode ?? s.PlaybackMode;
    public string DisplayedMode(Snapshot s) => Requested ?? PreferredMode(s);
    public string BitrateMode(Snapshot s) => Requested ?? s.PlaybackMode;

    public SetPlaybackModeCmd? Begin(Snapshot s, string mode)
    {
        if (mode == DisplayedMode(s)) return null;
        Requested = mode;
        return new SetPlaybackModeCmd { Mode = mode };
    }

    public void Update(Snapshot s)
    {
        if (PreferredMode(s) == Requested) Requested = null;
    }

    public void Cancel() => Requested = null;
    public static PlaybackBitrate BitrateFor(Snapshot s, string mode) =>
        mode == "aac_eld" ? s.PlaybackAacEldBitrate : s.PlaybackAacBitrate;
}
