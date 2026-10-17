using NTPods.WinUI.Ipc;
using NTPods.WinUI.Popup;
using NTPods.WinUI.Services;

internal static class IslandTakeoverChecks
{
    public static void Run(Action<bool, string> check)
    {
        const string first = "001122334455";
        const string second = "aabbccddeeff";
        var snapshot = Nearby(first);
        var policy = new IslandTakeover();
        policy.ObserveSnapshot(snapshot, 0);
        check(policy.State(snapshot, 0) == IslandConnectState.Ready,
            "AACP control without an audio output still offers Connect.");

        policy.PairingSucceeded(first);
        check(!policy.MayShow(snapshot), "Pairing suppresses a duplicate connection island for that pair.");
        check(policy.ObserveLid(new DaemonEvent.PairedLid(second, false, 1), snapshot, 1) is null,
            "Another pair cannot clear suppression or fold this island.");
        check(policy.ObserveLid(new DaemonEvent.PairedLid(first, false, 1), snapshot, 2) == false,
            "An owned close with no visible island is accepted.");
        check(!policy.MayShow(snapshot), "A closed case cannot display stale nearby batteries.");
        check(policy.ObserveLid(new DaemonEvent.PairedLid(first, true, 2), snapshot, 3) == false &&
            policy.MayShow(snapshot), "A fresh open after pairing can display the nearby island.");
        policy.Shown(snapshot);
        check(policy.ObserveLid(new DaemonEvent.PairedLid(first, false, 2), snapshot, 4) is null,
            "A stale close cannot fold a newer opening.");
        check(policy.ObserveLid(new DaemonEvent.PairedLid(first, false, 3), snapshot, 5) == true,
            "A newer owned close folds its visible nearby island.");
        check(!policy.MayShow(snapshot), "A closed case ignores the following stale battery snapshot.");

        snapshot.AudioOutputReady = true;
        policy.ObserveSnapshot(snapshot, 100);
        policy.ObserveLid(new DaemonEvent.PairedLid(first, true, 4), snapshot, 101);
        policy.ObserveSnapshot(snapshot, 102);
        policy.Shown(snapshot);
        check(policy.ObserveLid(new DaemonEvent.PairedLid(first, false, 5), snapshot, 103) == true,
            "Closing a connected local pair folds its battery island.");
        snapshot.AudioOutputReady = false;
        policy.ObserveSnapshot(snapshot, 104);
        policy.ObserveLid(new DaemonEvent.PairedLid(first, true, 6), snapshot, 105);
        check(policy.InGrace(105) && policy.State(snapshot, 105) == IslandConnectState.Hidden,
            "A local close/reopen hides Connect during reconnect grace.");
        policy.ObserveLid(new DaemonEvent.PairedLid(first, false, 7), snapshot, 106);
        policy.ObserveLid(new DaemonEvent.PairedLid(first, true, 8), snapshot, 107);
        policy.Shown(snapshot);
        check(policy.State(snapshot, 4_105) == IslandConnectState.Ready,
            "Rapid close/reopen does not extend the original grace deadline.");

        check(policy.BeginConnect(snapshot, 4_106), "Connect starts once the grace period expires.");
        check(!policy.BeginConnect(snapshot, 4_107) && policy.State(snapshot, 4_107) == IslandConnectState.Connecting,
            "Repeated taps do not issue duplicate connect requests.");
        check(policy.State(snapshot, 34_106) == IslandConnectState.Failed,
            "An output that never appears produces a bounded Retry state.");
        check(policy.BeginConnect(snapshot, 34_107), "Retry can issue a new request.");
        snapshot.AudioOutputReady = true;
        policy.ObserveSnapshot(snapshot, 34_108);
        check(policy.State(snapshot, 34_108) == IslandConnectState.Hidden && !policy.NeedsTimer(34_108),
            "The selected audio output completes the request and hides Connect.");

        snapshot.AudioOutputReady = false;
        policy.ObserveSnapshot(snapshot, 34_109);
        check(policy.BeginConnect(snapshot, 34_110), "A later nearby session can start another request.");
        policy.MessageReplaced();
        check(policy.State(snapshot, 34_111) == IslandConnectState.Ready,
            "A mode notice retires the old interactive request.");
        policy.Shown(snapshot);
        check(policy.BeginConnect(snapshot, 34_112), "A new island can reconnect the same pair.");
        var switched = Nearby(second);
        policy.ObserveSnapshot(switched, 34_113);
        check(policy.State(switched, 34_113) == IslandConnectState.Ready &&
            !policy.BeginConnect(switched, 34_113),
            "A pair switch retires the old request and cannot connect from stale artwork.");
        policy.SessionLost();
        policy.ObserveSnapshot(switched, 34_114);
        check(policy.State(switched, 34_114) == IslandConnectState.Ready,
            "A different pair and daemon session cannot inherit the previous request.");
        Console.WriteLine("Island takeover checks passed.");
    }

    private static Snapshot Nearby(string id) => new()
    {
        DeviceId = id,
        Connected = true,
        BatteryLive = true,
        BatterySource = "ble",
        Battery = new Battery { Left = 85 }
    };
}
