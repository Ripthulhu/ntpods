using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class PlaybackModeChecks
{
    public static void Run(Action<bool, string> check)
    {
        var request = new PlaybackModeRequest();
        var fallback = new Snapshot
        {
            PlaybackPreferredMode = "aac_eld", PlaybackMode = "aac", PlaybackState = "ready",
            PlaybackAacBitrate = new() { BestEffort = false, Kbps = 320 },
            PlaybackAacEldBitrate = new() { BestEffort = true, Kbps = 192 }
        };
        check(request.DisplayedMode(fallback) == "aac_eld" && request.BitrateMode(fallback) == "aac",
            "AAC fallback must retain the ELD preference in the picker and expose the active AAC bitrate.");
        check(PlaybackModeRequest.BitrateFor(fallback, request.BitrateMode(fallback)).Kbps == 320,
            "Fallback bitrate controls must use the separately saved AAC value.");
        check(request.Begin(fallback, "aac") is { Mode: "aac" },
            "Choosing AAC during fallback must send a preference change even though AAC is already active.");
        request.Update(fallback);
        check(request.Requested == "aac", "The active fallback codec must not acknowledge a preference change.");
        fallback.PlaybackPreferredMode = "aac";
        request.Update(fallback);
        check(request.Requested is null && request.DisplayedMode(fallback) == "aac",
            "The saved AAC preference must acknowledge the request.");
        check(request.Begin(fallback, "aac") is null, "Selecting the saved preference must not restart playback.");

        check(request.Begin(fallback, "aac_eld") is { Mode: "aac_eld" }, "ELD remains selectable after saving AAC.");
        fallback.PlaybackPreferredMode = "aac_eld";
        request.Update(fallback);
        check(request.Requested is null && request.DisplayedMode(fallback) == "aac_eld" && request.BitrateMode(fallback) == "aac",
            "An ELD preference must finish pending UI state even when playback falls back to AAC.");

        fallback.PlaybackState = "error";
        fallback.PlaybackError = "The AirPods cannot use this AAC bitrate.";
        check(request.BitrateMode(fallback) == "aac" && !PlaybackModeRequest.BitrateFor(fallback, request.BitrateMode(fallback)).BestEffort,
            "An AAC fallback bitrate error must expose the offending AAC manual setting, not the ELD preference.");
        request.Begin(fallback, "aac");
        request.Cancel();
        check(request.Requested is null && request.DisplayedMode(fallback) == "aac_eld",
            "A timeout or failed send must restore the confirmed preference.");

        var nextPair = new Snapshot { PlaybackPreferredMode = "aac_eld", PlaybackMode = "aac_eld", PlaybackState = "ready" };
        request.Update(nextPair);
        check(request.DisplayedMode(nextPair) == "aac_eld" && request.BitrateMode(nextPair) == "aac_eld",
            "An ELD-capable pair must display its active codec while retaining the same preference.");
        var legacy = new Snapshot { PlaybackMode = "aac" };
        check(request.DisplayedMode(legacy) == "aac", "Older backends must use their existing mode as the preference.");
        request.Begin(legacy, "aac_eld");
        legacy.PlaybackMode = "aac_eld";
        request.Update(legacy);
        check(request.Requested is null, "Older backend mode reports must still acknowledge requests.");
        Console.WriteLine("Playback preference and fallback controls checks passed.");
    }
}
