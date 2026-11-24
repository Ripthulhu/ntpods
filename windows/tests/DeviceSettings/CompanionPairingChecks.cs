using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class CompanionPairingChecks
{
    private const string Device = "001122334455", Phone = "12:34:56:78:9A:BC", OtherPhone = "98:76:54:32:10:AB";
    private const string Code = "01234567";

    public static void Run(Action<bool, string> check)
    {
        check(CompanionSetupControls.ValidPairingCode(Code) && CompanionSetupControls.DisplayPairingCode(Code) == "0123 4567",
            "Eight-digit codes preserve leading zeroes and group only their display.");
        foreach (string invalid in new[] { "", "1234567", "123456789", "1234 5678", "１２３４５６７８", "1234567a" })
            check(!CompanionSetupControls.ValidPairingCode(invalid), "The pairing wire accepts exactly eight ASCII digits.");

        var setup = Ready();
        var begin = setup.BeginPairing(Phone, Code, 1, 100);
        check(begin is { RequestId: 1, Code: Code, DeviceId: Device, PeerAddress: Phone } && setup.PairingRequestId == 1,
            "Short-code setup binds the code to its original request, phone and selected dongle pair.");
        setup.Apply(Status(1, "waiting", 119_900), 200);
        check(!setup.CanRevealCode(Device, Phone, Phone, true, 200), "Window status without its begin acknowledgement cannot reveal the code.");
        check(!setup.Complete(Result("companion_link", 1), 1) && !setup.Complete(Result("companion_begin_pairing", 2), 1) &&
            !setup.Complete(Result("companion_begin_pairing", 1), 2), "Another command, request or epoch cannot acknowledge a short-code window.");
        check(setup.Complete(Result("companion_begin_pairing", 1), 1) && setup.AwaitingStatus &&
            !setup.CanRevealCode(Device, Phone, Phone, true, 250), "A fresh window status must follow the matching begin acknowledgement.");
        setup.Apply(Status(1, "waiting", 119_800), 300);
        check(setup.CanRevealCode(Device, Phone, Phone, true, 301) && !setup.CanLink && !setup.CanForget &&
            setup.Status is { Linked: true, DeviceId: "aabbccddeeff", PeerAddress: OtherPhone },
            "A confirmed short-code window can show its code while the previous approved link remains unchanged.");
        check(!setup.CanRevealCode(Device, Phone, OtherPhone, true, 301) &&
            !setup.CanRevealCode("aabbccddeeff", Phone, Phone, true, 301) &&
            !setup.CanRevealCode(Device, Phone, Phone, false, 301), "Picker, pair and reopened-dialog mismatches keep the code private.");
        setup.Apply(Status(1, "authenticating", 119_000), 400);
        long remaining = setup.RemainingPairingMs(500);
        setup.Apply(Status(1, "authenticating", 120_000), 500);
        check(setup.PairingPhase == "authenticating" && setup.RemainingPairingMs(500) == remaining,
            "Heartbeat statuses cannot extend the fixed local expiry.");
        setup.Apply(Status(2, "waiting", 120_000), 600);
        check(setup.PairingPhase == "cancelled" && !setup.CanRevealCode(Device, Phone, Phone, true, 600) &&
            setup.CancelPairingCommand(1) is null, "A replacement window cannot confirm the old code or receive its cancellation.");

        check(setup.BeginPairing(Phone, Code, 3, 1_000) is not null, "A replacement window requires an explicit new request.");
        var oldCancel = setup.CancelPairingCommand(1);
        check(oldCancel?.RequestId == 3 && setup.CancelPairingCommand(2) is null,
            "Cancellation carries the original window ID and cannot cross daemon epochs.");
        setup.Dismiss();
        check(!setup.Complete(Result("companion_begin_pairing", 3), 1) && setup.BeginPairing(Phone, Code, 4, 1_100) is not null &&
            setup.CancelPairingCommand(1)?.RequestId == 4 && oldCancel?.RequestId == 3,
            "Closing a window retires its acknowledgement; its captured cancellation cannot target a newer window.");
        check(!setup.Complete(Result("companion_cancel_pairing", 3), 1) && setup.Pending == 4,
            "A late cancel receipt cannot complete the newer begin request.");
        setup.SetSession(2, false);
        check(setup.PairingRequestId is null && !setup.CanRevealCode(Device, Phone, Phone, true, 1_200),
            "Session loss retires short-code state without retaining a reconnect request.");

        var expiry = Ready();
        expiry.BeginPairing(Phone, Code, 9, 1_000);
        expiry.Complete(Result("companion_begin_pairing", 9), 1);
        expiry.Apply(Status(9, "waiting", 120_000), 1_001);
        check(expiry.CanRevealCode(Device, Phone, Phone, true, 120_999) && expiry.ExpirePairing(121_000),
            "The code is usable only before the original two-minute deadline.");
        expiry.Apply(Status(9, "waiting", 120_000), 121_001);
        check(expiry.PairingPhase == "expired" && !expiry.CanRevealCode(Device, Phone, Phone, true, 121_001) && expiry.CanLink,
            "Late heartbeats cannot revive an expired code; renewal requires an explicit action.");
        expiry.BeginPairing(Phone, Code, 10, 200_000);
        expiry.Complete(Result("companion_begin_pairing", 10), 1);
        expiry.Apply(Status(10, "completed", 0, committed: true), 200_001);
        check(expiry.PairingPhase == "completed" && expiry.CancelPairingCommand(1) is null &&
            !expiry.CanRevealCode(Device, Phone, Phone, true, 200_001) && expiry.Status is { Linked: true, DeviceId: Device },
            "Successful authentication clears the code and cannot be cancelled as an uncommitted setup window.");
        expiry.Dismiss();
        check(expiry.Status is { Linked: true, DeviceId: Device }, "Dismissing completed setup never forgets the new approval.");

        var timeout = Ready();
        timeout.BeginPairing(Phone, Code, 11, 0);
        var timedOutCancel = timeout.CancelPairingCommand(1);
        timeout.SendFailed();
        timeout.Apply(Status(11, "waiting", 120_000), 10_000);
        check(timedOutCancel?.RequestId == 11 && !timeout.Complete(Result("companion_begin_pairing", 11), 1) &&
            !timeout.CanRevealCode(Device, Phone, Phone, true, 10_000), "A begin timeout retires disclosure even if late status and acknowledgement arrive.");
        timeout.Apply(Status(11, "completed", 0, committed: true), 10_001);
        check(timeout.PairingPhase == "completed" && !timeout.Failed && timeout.CancelPairingCommand(1) is null &&
            !timeout.CanRevealCode(Device, Phone, Phone, true, 10_001),
            "A confirmed commit after local timeout updates the outcome without reviving the code or undoing the new link.");

        var statusExpiry = Ready();
        statusExpiry.BeginPairing(Phone, Code, 20, 0);
        statusExpiry.Complete(Result("companion_begin_pairing", 20), 1);
        statusExpiry.Apply(Status(20, "authenticating", 120_000), 100);
        var expiryCancel = statusExpiry.Apply(Status(20, "authenticating", 5_000), 120_001);
        check(expiryCancel?.RequestId == 20 && statusExpiry.PairingPhase == "expired" && !statusExpiry.PairingActive &&
            statusExpiry.CancelPairingCommand(1) is null,
            "Status-triggered local expiry captures the original cancellation before retiring the active window.");
        check(statusExpiry.Apply(Status(20, "waiting", 5_000), 120_002) is null,
            "A repeated expired status cannot queue duplicate cancellations or revive the code.");
        statusExpiry.BeginPairing(Phone, Code, 21, 120_003);
        statusExpiry.Complete(Result("companion_begin_pairing", 21), 1);
        check(statusExpiry.Apply(Status(21, "completed", 0, committed: true), 240_004) is null &&
            statusExpiry.PairingPhase == "completed" && expiryCancel?.RequestId == 20,
            "Completed setup is never cancelled even after the local deadline; an old expiry cancellation retains its original ID.");

        var errors = Ready();
        errors.BeginPairing(Phone, Code, 30, 0);
        errors.SendFailed();
        check(errors.DialogErrorKey == "CompanionSetup_SaveFailed", "A failed attempt exposes its current operation error.");
        errors.BeginPairing(Phone, Code, 31, 100);
        check(!errors.Failed && errors.DialogErrorKey is null,
            "Starting an explicit retry removes the failed attempt's dialog error immediately.");
        errors.Complete(Result("companion_begin_pairing", 31), 1);
        errors.Apply(Status(31, "authenticating", 119_000), 200);
        check(errors.PairingPhase == "authenticating" && errors.DialogErrorKey is null,
            "Authentication progress remains visible without the previous attempt's stale error.");
        errors.SetDialogError("CompanionSetup_CopyFailed");
        errors.Apply(Status(31, "authenticating", 118_000), 300);
        check(errors.DialogErrorKey == "CompanionSetup_CopyFailed" && errors.PairingPhase == "authenticating",
            "A current clipboard error remains actionable without replacing authentication progress.");
        errors.Apply(Status(31, "completed", 0, committed: true), 400);
        check(errors.DialogErrorKey is null && errors.PairingPhase == "completed",
            "Confirmed completion clears transient dialog errors instead of displaying a false failure.");
        errors.SetDialogError("CompanionSetup_LoadFailed");
        errors.Dismiss();
        check(errors.DialogErrorKey is null, "Reopening setup cannot inherit a previous dialog's discovery error.");
        Console.WriteLine("Short-code pairing window, acknowledgement, expiry and cancellation checks passed.");
    }

    private static CompanionSetupControls Ready()
    {
        var setup = new CompanionSetupControls();
        setup.SetSession(1, true);
        setup.Update(new Snapshot { DeviceId = "aabbccddeeff", Dongle = new DongleState { Present = true, Selected = true, Session = 1, PairedDeviceId = Device } });
        setup.Apply(new() { Linked = true, DeviceId = "aabbccddeeff", PeerAddress = OtherPhone });
        return setup;
    }

    private static CompanionStatus Status(ulong request, string phase, uint remaining, bool committed = false) => new()
    {
        Linked = true, DeviceId = committed ? Device : "aabbccddeeff", PeerAddress = committed ? Phone : OtherPhone,
        Pairing = new() { RequestId = request, DeviceId = Device, PeerAddress = Phone, Phase = phase, RemainingMs = remaining }
    };
    private static DaemonEvent.CommandResult Result(string command, ulong request) => new(command, request, true, "Fixture result");
}
