using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class CompanionChecks
{
    private const string First = "001122334455";
    private const string Second = "aabbccddeeff";

    public static void Run(Action<bool, string> check)
    {
        var controls = new CompanionControls();
        var snapshot = new Snapshot { DeviceId = First, Connected = false };
        controls.Update(snapshot);
        controls.Apply(Status());
        check(!controls.Visible && !controls.CanSwitch && !controls.IsSwitching && controls.Begin("android", 1) is null,
            "Companion status cannot authorize a transfer before a ready daemon session.");
        controls.SetSession(1, true);
        controls.Apply(Status(heldAway: true));
        check(controls.Visible && controls.CanSwitch && !controls.IsSwitching,
            "A linked current phone can return audio even while PC audio is disconnected and held away.");

        foreach (string invalid in new[] { "", "000000000000", "00:11:22:33:44:55", "00112233445g" })
        {
            controls.Update(new Snapshot { DeviceId = invalid });
            controls.Apply(Status(invalid));
            check(!controls.Visible && controls.Begin("pc", 1) is null,
                "An invalid or missing selected pair must not expose companion transfer controls.");
        }
        controls.Update(snapshot);
        controls.Apply(Status(Second));
        check(!controls.Visible, "A different pair's linked phone must not authorize this pair.");
        controls.Apply(new CompanionStatus());
        check(!controls.Visible, "An unconfigured companion remains hidden.");
        controls.Update(new Snapshot { DeviceId = Second });
        controls.Apply(Status(Second.ToUpperInvariant()));
        check(controls.CanSwitch, "Canonical device identifiers compare without case sensitivity.");
        controls.Update(snapshot);
        controls.Apply(Status());

        foreach (string target in new[] { "phone", "", "PC" })
            check(controls.Begin(target, 2) is null && controls.Pending is null,
                "Only the wire destinations pc and android can create a transfer.");
        check(controls.Begin("pc", 0) is null, "Zero is not a valid request identity.");
        var command = controls.Begin("android", ulong.MaxValue);
        check(command is { DeviceId: First, Target: "android", RequestId: ulong.MaxValue } &&
            controls.Pending == ulong.MaxValue && !controls.CanSwitch && controls.IsSwitching,
            "A transfer preserves the selected pair, destination and full-width request ID.");
        check(controls.Begin("pc", 3) is null && controls.HelpKey == "Companion.Pending",
            "A pending transfer blocks a second tap.");
        foreach (var wrong in new[] {
            Result("connect", ulong.MaxValue, true), Result("companion_switch", 9, true) })
            check(!controls.Complete(wrong, 1) && controls.Pending == ulong.MaxValue,
                "An unrelated result must not retire the pending transfer.");
        check(!controls.Complete(Result("companion_switch", ulong.MaxValue, true), 2),
            "A replacement session's result cannot acknowledge an old transfer.");
        controls.SetSession(1, true);
        check(controls.Pending == ulong.MaxValue, "An unchanged ready session preserves its pending transfer.");
        check(controls.Complete(Result("companion_switch", ulong.MaxValue, true), 1) &&
            controls.Pending is null && controls.ErrorKey is null && controls.CanSwitch && !controls.IsSwitching,
            "Only the matching terminal result releases the current transfer.");

        controls.Begin("pc", 4);
        check(controls.Complete(Result("companion_switch", 4, false), 1) &&
            controls.ErrorKey == "Companion.Failed" && controls.CanSwitch,
            "A terminal failure gives a bounded retry state without claiming route ownership.");
        controls.Begin("pc", 5);
        controls.SendFailed();
        check(controls.Pending is null && controls.ErrorKey == "Companion.Failed",
            "A failed pipe send must release the local latch and show failure.");

        foreach (string phase in new[] { "prepare", "release_android", "release_pc", "acquire_android", "acquire_pc", "unknown" })
        {
            controls.Apply(Status(phase: phase));
            check(!controls.CanSwitch && controls.Begin("android", 6) is null,
                "An active or unrecognized daemon phase cannot accept another transfer.");
            check(controls.IsSwitching == (phase != "unknown"),
                "Only recognized transfer phases show progress when no local request is pending.");
        }
        controls.Apply(Status(phase: "acquire_pc", connected: false));
        check(!controls.IsSwitching,
            "A disconnected companion's cached transfer phase must not show live progress.");
        controls.Apply(Status(connected: false));
        check(controls.Visible && !controls.CanSwitch, "A configured but disconnected phone remains unavailable.");
        controls.Apply(Status(phase: "waiting"));
        check(controls.CanSwitch && !controls.IsSwitching, "A connected waiting companion can accept an explicit transfer.");

        foreach (var busy in new[] {
            new Snapshot { DeviceId = First, MicCaptureActive = true },
            new Snapshot { DeviceId = First, MicUplinkActive = true },
            new Snapshot { DeviceId = First, MicState = "recovering" },
            new Snapshot { DeviceId = First, PlaybackBufferTargetMs = 20 } })
        {
            controls.Update(busy);
            check(!controls.CanSwitch && controls.HelpKey == "Companion.Busy",
                "An active local audio operation blocks companion transfer.");
        }
        controls.Update(snapshot, localBusy: true);
        check(!controls.CanSwitch, "A local control operation also blocks transfer.");
        controls.Update(snapshot);
        controls.Begin("android", 7);
        controls.Update(new Snapshot { DeviceId = Second });
        check(controls.Pending is null && !controls.Visible &&
            !controls.Complete(Result("companion_switch", 7, true), 1),
            "Changing the selected pair retires its pending result and linked status.");
        controls.Apply(Status(Second));
        controls.Begin("pc", 8);
        controls.SetSession(2, true);
        check(controls.Pending is null && !controls.Visible && !controls.CanSwitch,
            "A new daemon epoch requires fresh companion status.");
        controls.Apply(Status(Second));
        check(controls.CanSwitch, "Fresh status can enable the replacement session.");
        controls.SetSession(2, false);
        controls.Apply(Status(Second));
        check(!controls.Visible && controls.Pending is null && controls.ErrorKey is null && !controls.IsSwitching,
            "Session loss clears status, errors and pending transfers without replay.");

        var recovery = new CompanionControls();
        recovery.Update(snapshot);
        recovery.SetSession(3, true);
        recovery.Apply(Status(), 0);
        check(recovery.Begin("android", 100, 100) is not null && recovery.IsSwitching && !recovery.Expire(80_099),
            "The UI recovery deadline must preserve an in-flight request for its full 80-second budget.");
        recovery.Apply(Status(phase: "acquire_android"), 80_099);
        check(recovery.Expire(80_100) && recovery.Pending is null && !recovery.CanSwitch &&
            recovery.ErrorKey == "Companion.Failed" && !recovery.IsSwitching,
            "A missing terminal receipt must release Switching once and require fresh status before another request.");
        check(!recovery.Expire(90_000) && !recovery.Complete(Result("companion_switch", 100, true), 3),
            "Recovery cannot repeatedly expire or accept a late result for its retired request.");
        recovery.Apply(Status(), 80_099);
        check(!recovery.CanSwitch && recovery.Begin("pc", 101, 90_000) is null,
            "An idle status received before expiry but queued behind it cannot unlock recovery.");
        recovery.Apply(Status(), 80_100);
        check(!recovery.CanSwitch,
            "A queued status in the same coarse clock tick as expiry is not proof of a newer status receipt.");
        recovery.Apply(Status(phase: "acquire_android"), 90_001);
        check(!recovery.CanSwitch,
            "Fresh status showing the daemon still transferring must continue to block a new request.");
        recovery.Apply(Status(), 90_002);
        check(recovery.CanSwitch && recovery.ErrorKey == "Companion.Failed",
            "A fresh idle status permits a new explicit request while retaining the unconfirmed-result message.");
        check(recovery.Begin("pc", 101, 90_003) is not null &&
            !recovery.Complete(Result("companion_switch", 100, true), 3) && recovery.Pending == 101,
            "A late terminal receipt cannot complete the explicit retry that follows recovery.");
        check(recovery.Complete(Result("companion_switch", 101, true), 3) && recovery.CanSwitch &&
            recovery.ErrorKey is null && !recovery.Expire(170_003),
            "The retry's own receipt clears failure and retires its recovery deadline.");
        Console.WriteLine("Companion control checks passed.");
    }

    private static CompanionStatus Status(string device = First, string phase = "idle",
        bool connected = true, bool heldAway = false) => new()
        { Linked = true, Connected = connected, DeviceId = device, Phase = phase, HeldAway = heldAway };

    private static DaemonEvent.CommandResult Result(string command, ulong id, bool ok) =>
        new(command, id, ok, "Fixture result");
}
