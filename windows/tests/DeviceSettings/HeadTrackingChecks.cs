using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class HeadTrackingChecks
{
    public static void Run(Action<bool, string> check)
    {
        var state = new HeadTrackingLiveState();
        var connected = new Snapshot { Connected = true, HeadTrackingEnabled = true, HeadTrackingState = "streaming" };
        check(!state.Add(new(1_000, 50, -50), 1_000), "Unrequested movement must not enter the chart.");
        state.Update(connected);
        check(state.Add(new(1_000, short.MinValue, short.MaxValue), 1_000), "Signed sensor extremes must remain valid.");
        check(state.Range > 32_768 && state.IsFresh(2_999) && !state.IsFresh(3_000), "Motion scale and freshness must be bounded.");
        check(!state.Add(new(1_000, 1, 2), 1_001) && !state.Add(new(999, 1, 2), 1_001), "Duplicates and reordered events must be ignored.");
        check(!state.Add(new(2_000, 1, 2), 1_001) && !state.Add(new(1_001, 1, 2), 3_001), "Future or delayed motion must not look current.");
        for (ulong i = 1_001; i <= 1_110; ++i) state.Add(new(i, (short)i, (short)-(short)i), i);
        check(state.Samples.Count == 100 && state.Samples[0].TimestampMs == 1_011 && Math.Abs(state.Range - 1_332) < 0.01,
            "The plot must retain only its latest 100 samples and rescale after a peak leaves.");
        connected.HeadTrackingEnabled = false;
        state.Update(connected);
        check(!state.IsFresh(1_111) && !state.Add(new(1_111, 1, 2), 1_111), "Paused history must not accept late movement.");
        connected.HeadTrackingEnabled = true;
        state.Update(connected);
        check(state.Samples.Count == 0, "A new tracking session must begin without the old trace.");
        state.Add(new(2_000, 3, 4), 2_000);
        state.Update(new Snapshot());
        check(state.Samples.Count == 0 && !state.IsFresh(2_000), "Disconnect must clear the previous device's movement.");
        Console.WriteLine("Head tracking history, freshness and bounded plotting checks passed.");
    }
}
