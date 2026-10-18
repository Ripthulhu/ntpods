using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

internal static class NearbyFinderChecks
{
    public static void Run(Action<bool, string> check)
    {
        const string first = "001122334455", second = "aabbccddeeff";
        var view = new NearbyFinderPresentation();
        view.Update(new Snapshot { DeviceId = first }, 0);
        check(!view.CanStart(true) && view.StatusKey(true, 0) == "NearbyFinder.Unavailable",
            "Older daemons must not offer a finder command they do not support.");
        view.Update(State(first, identity: false), 0);
        check(!view.CanStart(true) && view.StatusKey(true, 0) == "NearbyFinder.NoIdentity",
            "A selected pair without an identity key cannot start a search.");
        view.Update(State(first), 0);
        check(view.CanStart(true) && !view.CanStart(false) && view.Target is null,
            "Snapshots may enable Start, but must never start a search themselves.");
        view.Started(10);
        check(view.Target == first && view.Starting && !view.CanStart(true),
            "A pending Start belongs to its selected pair and disables duplicate starts.");
        view.Update(State(first), 20);
        check(view.Starting, "A preceding idle snapshot must not cancel an unacknowledged Start.");
        check(view.ExpireStart(5_009) is null && view.ExpireStart(5_010) == first && view.ExpireStart(5_011) is null,
            "A timed-out Start must return its target for Stop exactly once.");
        view.Started(6_000);
        check(view.Update(State(second), 6_001) == first && view.Target is null,
            "Changing pairs must stop the previous target without transferring ownership.");
        view.Update(State(first), 7_000);
        view.Started(7_001);
        view.Update(State(first, active: true, rssi: -55, age: 100, trend: "stronger"), 7_002);
        check(!view.Starting && view.Target == first && view.StatusKey(true, 7_002) == "NearbyFinder.Strong" &&
            view.TrendKey(7_002) == "NearbyFinder.Stronger", "An acknowledged verified sample may show signal guidance.");
        check(view.Age(7_902) == 1_000 && view.LastHeardText(7_902) == "NearbyFinder.SecondsAgo 1",
            "Last-heard age must advance monotonically between snapshots.");
        check(view.Age(7_000) == 100, "A backwards timer must not invent a newer observation.");
        check(view.Fresh(16_902) && !view.Fresh(16_903) && view.TrendKey(16_903) == "" && view.Rssi == -55,
            "After ten seconds, guidance must dim while retaining the last known signal.");
        check(view.StatusKey(true, 67_002) == "NearbyFinder.NoRecentSignal" &&
            view.LastHeardText(67_002) == "NearbyFinder.MinutesAgo 1",
            "A quiet closed case must retain its real age without a lost-device claim.");
        view.Update(State(first, active: true, paused: true, rssi: -55, age: 0, trend: "stronger"), 70_000);
        check(view.StatusKey(true, 70_000) == "NearbyFinder.Paused" && !view.Fresh(70_000) && view.TrendKey(70_000) == "",
            "Microphone pauses must not leave live guidance on screen.");
        view.Update(State(first, active: true, rssi: 127, age: 0, trend: "weaker"), 80_000);
        check(view.Rssi is null && view.Signal == 0 && view.TrendKey(80_000) == "",
            "Unavailable RSSI values must not appear as maximum signal strength.");
        view.Update(State(first, active: true, rssi: -127, age: 0), 80_001);
        check(view.Rssi is null && !view.Fresh(80_001),
            "Windows' synthetic out-of-range RSSI must not count as a fresh signal.");
        view.Update(State(first, active: true, rssi: -126, age: 0), 80_002);
        check(view.Rssi == -126 && view.Fresh(80_002), "The lowest valid RSSI remains usable.");
        view.Update(State(first, active: true, rssi: -90, age: ulong.MaxValue, trend: "future"), 90_000);
        check(view.Age(100_000) == ulong.MaxValue && view.TrendKey(100_000) == "",
            "Invalid ages must not overflow into fresh readings or invent trend labels.");
        check(view.Stop() == first && view.Stop() is null && view.Target is null,
            "Collapse, unload and navigation may only release this view's session once.");
        check(view.StatusKey(true, 100_000) == "NearbyFinder.Busy" && !view.CanStart(true),
            "An active session without local ownership must not expose another client's Stop action.");
        view.Update(State(first), 110_000);
        view.Started(110_001);
        view.Update(State(first, active: true, rssi: -55, age: 100, revision: 2), 110_002);
        view.Update(State(first, revision: 1), 110_003);
        check(view.Target == first && !view.Starting && !view.CanStart(true),
            "A delayed inactive snapshot must preserve the owned session's Stop action.");
        check(view.Rssi == -55 && view.Age(110_003) == 101,
            "An older revision must not replace signal data or reset its age.");
        view.Update(State(first, active: true, revision: 3), 110_004);
        check(view.Target == first && view.StatusKey(true, 110_004) != "NearbyFinder.Busy" &&
            view.ExpireStart(120_000) is null,
            "A later active snapshot must remain locally owned after an older inactive report.");
        check(view.Stop() == first && view.Target is null,
            "The retained session must still stop explicitly with its original device ID.");
        view.Update(State(first, revision: 2), 110_005);
        check(!view.CanStart(true), "Explicit Stop must retain the daemon's revision watermark.");
        view.Started(110_006);
        view.Update(State(first, active: true, revision: 4), 110_007);
        view.Update(State(first, revision: 5), 110_008);
        check(view.Target is null && view.CanStart(true), "A newer inactive state must release local ownership.");
        view.Reset();
        view.Update(State(first, revision: 1), 110_009);
        check(view.CanStart(true), "The same pair must accept lower revisions after the daemon connection resets.");
        view.Started(110_010);
        check(view.Update(State(second), 110_011) == first && view.CanStart(true),
            "Changing pairs must accept its state even with a lower revision.");
        view.Update(State(""), 120_000);
        check(!view.CanStart(true), "An empty selection cannot start a search.");
        Console.WriteLine("Nearby finder presentation and ownership checks passed.");
    }

    private static Snapshot State(string id, bool identity = true, bool active = false, bool paused = false,
        short? rssi = null, ulong? age = null, string trend = "unknown", ulong revision = 0) => new()
    {
        DeviceId = id,
        NearbyFinder = new()
        {
            Revision = revision, AvailableIdentity = identity, Active = active, Paused = paused,
            Rssi = rssi, Trend = trend, LastHeardAgeMs = age,
        },
    };
}
