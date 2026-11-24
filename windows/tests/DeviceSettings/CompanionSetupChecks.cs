using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class CompanionSetupChecks
{
    public static void Run(Action<bool, string> check)
    {
        const string first = "001122334455", second = "aabbccddeeff", phone = "12:34:56:78:9A:BC";
        string secret = new('a', 64);
        var controls = new CompanionSetupControls();
        var snapshot = new Snapshot { DeviceId = second, Dongle = new DongleState {
            Present = true, Selected = true, Session = 1, PairedDeviceId = first } };
        controls.Update(snapshot);
        controls.Apply(new());
        check(!controls.CanLink && controls.BeginLink(phone, secret, 1) is null,
            "Offline setup must not queue approval for a later daemon session.");
        controls.SetSession(3, true);
        check(!controls.CanLink, "Setup needs authoritative status before replacing an approval.");
        controls.Apply(new());
        check(controls.CanLink && !controls.CanForget && controls.DeviceId == first,
            "New setup targets the selected dongle pair, not the native Windows pair.");
        foreach (string invalid in new[] { "", new string('a', 63), new string('a', 65), new string('g', 64), new string('a', 31) + " " + new string('a', 32) })
            check(CompanionSetupControls.NormalizeSecret(invalid) is null,
                "Invalid setup codes must not become approval commands.");
        foreach (string invalid in new[] { "", "123456789ABC", "12-34-56-78-9A-BC", "00:00:00:00:00:00", "FF:FF:FF:FF:FF:FF", "02:00:00:00:00:00", "00:11:22:33:44:55" })
            check(controls.BeginLink(invalid, secret, 1) is null && controls.Pending is null,
                "Setup requires a real phone address distinct from the selected AirPods.");
        check(controls.BeginLink(phone, secret, 0) is null, "New setup actions require nonzero request correlation.");
        var link = controls.BeginLink(phone.ToLowerInvariant(), "  " + secret.ToUpperInvariant() + "  ", ulong.MaxValue);
        check(link is { DeviceId: first, PeerAddress: phone, RequestId: ulong.MaxValue } && link.SecretHex == secret &&
            controls.Pending == ulong.MaxValue && !controls.CanLink && !controls.CanForget,
            "A valid approval carries its exact pair, canonical code and full-width request identity.");
        check(!controls.CanRevealCode(first, phone, phone, true),
            "A code cannot be revealed while its approval request is pending.");
        controls.Apply(new());
        foreach (var stale in new[] { Result("companion_forget", ulong.MaxValue, true), Result("companion_link", 1, true) })
            check(!controls.Complete(stale, 3) && controls.Pending == ulong.MaxValue,
                "Unrelated setup results cannot reveal a code as saved.");
        check(!controls.Complete(Result("companion_link", ulong.MaxValue, true), 2),
            "A previous daemon epoch cannot acknowledge current setup.");
        check(controls.Complete(Result("companion_link", ulong.MaxValue, true), 3) && !controls.Failed &&
            controls.AwaitingStatus && !controls.CanLink && controls.Status is { Linked: false },
            "Only the matching successful acknowledgement confirms saving; link state still comes from the daemon.");
        check(!controls.CanRevealCode(first, phone, phone, true),
            "An acknowledgement alone cannot reveal a code before the following authoritative status.");
        controls.Apply(new() { Linked = true, DeviceId = first, PeerAddress = phone });
        check(controls.CanForget && controls.Status?.PeerAddress == phone && !controls.AwaitingStatus,
            "Fresh approval status enables explicit link management without needing an AirPods audio connection.");
        check(controls.CanRevealCode(first, phone, phone, true) &&
            !controls.CanRevealCode(first, phone, phone, false),
            "The confirmed code is visible only in the setup operation that received its successful acknowledgement, not a reopened dialog.");
        check(!controls.CanRevealCode(first, phone, "98:76:54:32:10:AB", true) &&
            !controls.CanRevealCode(first, phone, null, true) &&
            !controls.CanRevealCode(second, phone, phone, true) &&
            !controls.CanRevealCode(first, "98:76:54:32:10:AB", phone, true),
            "Changing either selected host or prepared pair cannot reveal the previous operation's code.");
        controls.Apply(new() { Linked = true, DeviceId = first, PeerAddress = "98:76:54:32:10:AB" });
        check(!controls.CanRevealCode(first, phone, phone, true),
            "A status for another phone cannot confirm the generated code.");
        controls.Apply(new() { Linked = true, DeviceId = second, PeerAddress = phone });
        check(!controls.CanRevealCode(first, phone, phone, true),
            "A status for another AirPods pair cannot confirm the generated code.");
        controls.Apply(new() { Linked = true, DeviceId = first, PeerAddress = phone });
        check(controls.BeginForget(12)?.RequestId == 12 &&
            controls.Complete(Result("companion_forget", 12, false), 3) && controls.Failed && controls.CanForget,
            "A failed forget result is correlated and leaves the existing approval available for explicit retry.");
        check(controls.BeginLink(phone, secret, 13) is not null && !controls.Failed,
            "An explicit new attempt clears the previous operation's failure.");
        snapshot.Dongle.PairedDeviceId = second;
        controls.Update(snapshot);
        check(controls.Pending is null && controls.DeviceId == second &&
            !controls.Complete(Result("companion_link", 13, true), 3),
            "Changing the selected AirPods retires the prior setup action and its delayed result.");
        check(controls.BeginLink(phone, secret, 14) is not null, "The replacement pair can be linked explicitly.");
        controls.SetSession(4, true);
        check(controls.Pending is null && controls.Status is null && !controls.CanLink &&
            !controls.Complete(Result("companion_link", 14, true), 3),
            "A replacement daemon session never replays or confirms the former session's setup.");
        controls.Apply(new() { Linked = true, DeviceId = first, PeerAddress = phone });
        snapshot.Dongle.Ambiguous = true;
        controls.Update(snapshot);
        check(!controls.CanLink && controls.CanForget,
            "Ambiguous dongles prevent provisioning another pair but do not prevent explicitly forgetting an existing approval.");
        check(controls.BeginForget(15) is not null, "An existing approval can be forgotten without a selected audio route.");
        controls.SendFailed();
        check(controls.Pending is null && controls.Failed && controls.CanForget,
            "A local send failure retires the setup request without reporting success.");
        check(!controls.CanRevealCode(first, phone, phone, true),
            "A timeout or send failure prevents code disclosure even if a caller retains its acknowledgement flag.");
        controls.SetSession(4, false);
        check(controls.Status is null && !controls.CanForget && !controls.Complete(Result("companion_forget", 15, true), 4) &&
            !controls.CanRevealCode(first, phone, phone, true),
            "Pipe loss clears cached approval state and cannot complete a queued forget.");
        var timedOut = new CompanionSetupControls();
        timedOut.SetSession(5, true);
        timedOut.Update(new Snapshot { DeviceId = first });
        timedOut.Apply(new());
        check(timedOut.BeginLink(phone, secret, 22) is not null &&
            timedOut.Complete(Result("companion_link", 22, true), 5) && timedOut.AwaitingStatus,
            "The timeout fixture waits for status after receiving its matching acknowledgement.");
        timedOut.SendFailed();
        timedOut.Apply(new() { Linked = true, DeviceId = first, PeerAddress = phone });
        check(!timedOut.CanRevealCode(first, phone, phone, true) &&
            !timedOut.Complete(Result("companion_link", 22, true), 5),
            "A late matching status or acknowledgement cannot revive a code retired by the status timeout.");
        check(timedOut.BeginLink(phone, secret, 23) is not null &&
            timedOut.Complete(Result("companion_link", 23, true), 5),
            "A subsequent explicit operation has its own acknowledgement.");
        timedOut.Apply(new() { Linked = true, DeviceId = first, PeerAddress = phone });
        check(timedOut.CanRevealCode(first, phone, phone, true), "The subsequent operation can reveal its confirmed code.");
        timedOut.SetSession(5, false);
        check(!timedOut.CanRevealCode(first, phone, phone, true),
            "Losing the daemon session invalidates disclosure even with the prepared pair unchanged.");
        var dismissed = new CompanionSetupControls();
        dismissed.SetSession(6, true);
        dismissed.Update(new Snapshot { DeviceId = first });
        var approved = new CompanionStatus { Linked = true, DeviceId = first, PeerAddress = phone };
        dismissed.Apply(approved);
        check(dismissed.BeginLink(phone, secret, 30) is not null, "The dismissal fixture starts a new approval.");
        dismissed.Dismiss();
        check(dismissed.CanLink && dismissed.CanForget && !dismissed.Failed && dismissed.Pending is null &&
            ReferenceEquals(dismissed.Status, approved) && !dismissed.Complete(Result("companion_link", 30, true), 6) &&
            !dismissed.CanRevealCode(first, phone, phone, true),
            "Dismissing a pending setup preserves the daemon session and status but rejects late acknowledgement and code disclosure.");
        check(dismissed.BeginLink(phone, secret, 31) is not null &&
            dismissed.Complete(Result("companion_link", 31, true), 6) && dismissed.AwaitingStatus,
            "A new explicit setup can await status independently of the dismissed request.");
        dismissed.Dismiss();
        dismissed.Apply(approved);
        check(dismissed.CanLink && !dismissed.AwaitingStatus && !dismissed.Complete(Result("companion_link", 31, true), 6) &&
            !dismissed.CanRevealCode(first, phone, phone, true),
            "Dismissing the acknowledgement-to-status wait prevents late matching status from reviving the old code.");
        check(dismissed.BeginLink(phone, secret, 32) is not null && dismissed.Complete(Result("companion_link", 32, true), 6),
            "Dismissal does not block a subsequent explicit retry in the same daemon epoch.");
        dismissed.Apply(approved);
        check(dismissed.CanRevealCode(first, phone, phone, true), "The retry can disclose only its own confirmed code.");
        dismissed.Dismiss();
        check(!dismissed.CanRevealCode(first, phone, phone, true) && dismissed.CanForget,
            "Closing a confirmed setup also retires its disclosure receipt without forgetting the approval.");
        dismissed.SendFailed();
        dismissed.Dismiss();
        check(!dismissed.Failed && dismissed.CanLink && ReferenceEquals(dismissed.Status, approved),
            "Dismissing an error clears presentation failure while preserving the authoritative approval.");
        Console.WriteLine("Companion setup validation and pair/session/request correlation checks passed.");
    }

    private static DaemonEvent.CommandResult Result(string command, ulong request, bool ok) => new(command, request, ok, "Fixture result");
}
