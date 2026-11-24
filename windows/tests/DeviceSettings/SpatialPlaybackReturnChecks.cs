using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class SpatialPlaybackReturnChecks
{
    public static void Run(Action<bool, string> check)
    {
        DongleState Route(string serial = "PICO-A", string? pair = "AABBCCDDEEFF", ulong session = 1) =>
            new() { Present = true, Serial = serial, Connected = true, Selected = true,
                PairedDeviceId = pair, Session = session, PairToken = session, UsbAudioAvailable = true };
        var recovery = new SpatialPlaybackReturn();
        var old = Route();
        var returned = Route(session: 2);
        recovery.Arm(old);
        recovery.Exited(100);
        recovery.Observe(old, 200);
        check(!recovery.Take(old, true), "An ordinary renderer exit cannot immediately relaunch playback.");
        recovery.Observe(new DongleState(), 700);
        check(recovery.Waiting && !recovery.Take(new(), false), "A missing dongle keeps Stop available while waiting.");
        recovery.Observe(returned, 800);
        check(!recovery.Take(returned, false) && recovery.Waiting, "Microphone use or an unavailable endpoint defers the return.");
        check(recovery.Take(returned, true) && !recovery.Take(returned, true),
            "The same hardware and pair can resume once across a changed USB session and pair token.");
        recovery.Arm(old);
        recovery.Observe(new(), 0);
        recovery.Observe(Route(pair: null, session: 2), 500);
        check(!recovery.Take(Route(pair: null, session: 2), true), "Unknown headphones cannot resume a saved route.");
        recovery.Observe(Route(pair: "001122334455", session: 2), 600);
        check(!recovery.Armed && !recovery.Take(returned, true), "Seeing another pair cancels the old route.");
        recovery.Arm(old);
        recovery.Observe(Route(serial: "PICO-B", session: 2), 100);
        check(!recovery.Armed, "Another selected Pico cancels recovery.");
        recovery.Arm(old);
        recovery.Observe(new() { Ambiguous = true }, 200);
        check(!recovery.Armed, "An ambiguous selection cannot inherit an active session.");
        recovery.Arm(old);
        recovery.Observe(new(), 0);
        recovery.Clear();
        recovery.Observe(returned, 800);
        check(!recovery.Armed && !recovery.Take(returned, true), "Stop or shutdown cancels a pending return.");
        recovery.Arm(old);
        recovery.Exited(100);
        recovery.Observe(new(), 5000);
        check(!recovery.Armed, "A subsequent unplug must not restart an unrelated failed renderer.");
        recovery.Arm(Route(pair: null));
        check(!recovery.Armed, "Unknown pair identity cannot arm automatic return.");
        recovery.Arm(Route(pair: "ffffffffffff"));
        check(!recovery.Armed, "An erased pairing record is not an identity.");
    }
}

