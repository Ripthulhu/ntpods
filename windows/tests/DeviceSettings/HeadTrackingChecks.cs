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
        CheckRoomStatus(check);
    }

    private static void CheckRoomStatus(Action<bool, string> check)
    {
        var room = new SpatialTrackingStatus();
        room.ReadPlayback("CAL_STATUS_LEFT_MISSING");
        check(room.PlaybackKey == null, "Calibration hints wait for the renderer to start.");
        room.ReadPlayback("SPATIAL_READY");
        check(room.PlaybackKey == "Spatial_CalibrateLeft", "Yaw-only tracking identifies the ear that needs calibration.");
        room.ReadPlayback("CAL_STATUS_RIGHT_READY");
        check(room.PlaybackKey == "Spatial_Playing", "A calibrated ear taking over clears the other ear’s prompt.");
        room.ReadPlayback("CAL_STATUS_RIGHT_MISSING");
        check(room.PlaybackKey == "Spatial_CalibrateRight", "A fresh missing profile identifies the current ear.");
        check(!room.ReadPlayback("CAL_STATUS_RIGHT_READY_EXTRA") && room.PlaybackKey == "Spatial_CalibrateRight",
            "Malformed status messages cannot dismiss a valid calibration hint.");
        room.ReadPlayback("CAL_STATUS_PENDING");
        check(room.PlaybackKey == "Spatial_Playing", "An unverified source clears the previous ear’s prompt.");
        room.ReadPlayback("CAL_STATUS_RIGHT_MISSING");
        room.ReadPlayback("SPATIAL_MOTION_WAIT");
        check(room.PlaybackKey == "Room_Recovering", "Motion recovery takes precedence over a calibration prompt.");
        room.ReadPlayback("SPATIAL_MOTION_READY");
        check(room.PlaybackKey == "Spatial_Playing", "Motion recovery does not revive an old calibration prompt.");
        room.ReadPlayback("SPATIAL_MOTION_WAIT");
        room.ReadPlayback("CAL_STATUS_LEFT_MISSING");
        check(room.PlaybackKey == "Room_Recovering", "A fresh calibration notice cannot hide an ongoing motion loss.");
        room.ReadPlayback("SPATIAL_MOTION_READY");
        check(room.PlaybackKey == "Spatial_CalibrateLeft", "Recovered motion displays the new ear’s calibration state.");
        room.ReadPlayback("CAL_STATUS_LEFT_READY");
        check(room.PlaybackKey == "Spatial_Playing", "A calibrated left ear resumes ordinary tracking status.");

        room.ReadSaved("CAL_SAVED_LEFT_EXTRA");
        check(room.SavedKey == null, "A malformed completion cannot count as a saved calibration.");
        room.ReadSaved("CAL_SAVED");
        check(room.SavedKey == "Spatial_Calibrated", "Older renderers keep their generic save confirmation.");
        foreach (var (line, key) in new[] { ("CAL_SAVED_LEFT", "Spatial_CalibratedLeft"), ("CAL_SAVED_RIGHT", "Spatial_CalibratedRight") })
        {
            var calibration = new SpatialTrackingStatus();
            check(calibration.PlaybackKey == null && calibration.SavedKey == null, "A new renderer session starts without previous status.");
            calibration.ReadSaved(line);
            calibration.ReadSaved("CAL_SAVED");
            check(calibration.SavedKey == key, "The generic confirmation preserves the saved ear.");
        }
    }
}
