using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class HearingAidChecks
{
    public static void Run(Action<bool, string> check)
    {
        var request = new HearingAidRequest();
        request.Update(false, 1);
        ulong id = request.Begin(true);
        check(request.Enabled == false && request.Requested, "A pending enable must not change confirmed state.");
        var conflict = new DaemonEvent.CommandResult("set_hearing_aid", id, false,
            "Turn off Custom Transparency before enabling hearing assistance.");
        check(request.Complete(conflict) && request.Enabled == false && request.Pending is null,
            "A rejected enable must restore the off switch and release pending state.");
        id = request.Begin(true);
        check(request.Complete(conflict) == false && request.Pending == id,
            "A late rejection must not cancel a newer request.");
        request.Update(false, 1);
        check(request.Pending == id, "A repeated cached Off must not cancel a new enable.");
        check(request.Complete(new("set_hearing_aid", id, true, "")) && request.Enabled == false,
            "Command success must not manufacture a hearing-state report.");
        request.Update(true, 2);
        check(request.Enabled == true, "A reported enable must update the switch even after its command completed.");
        id = request.Begin(false);
        check(request.Complete(new("set_hearing_aid", id, false, "Reconnect to retry.")) && request.Enabled == true,
            "A rejected disable must restore the previously enabled switch.");
        check(request.QueueEdit(), "Confirmed enabled state permits a deferred slider edit.");
        request.Update(false, 3);
        check(request.Enabled == false && !request.EditQueued && !request.TakeEdit(),
            "A reported Off must cancel a queued edit so it cannot re-enable hearing assistance.");
        id = request.Begin(true);
        request.Update(false, 4);
        check(request.Pending is null && !request.Complete(new("set_hearing_aid", id, true, "")) && request.Enabled == false,
            "A fresh Off during an enable must win over its late result.");
        request.Update(true, 5);
        check(request.QueueEdit(), "A later reported enable permits edits again.");
        request.Update(false, 4);
        check(request.Enabled == true && request.EditQueued, "An older snapshot cannot undo a newer report or queued edit.");
        check(request.TakeEdit() && !request.TakeEdit(), "A confirmed slider edit may be taken only once.");
        id = request.Begin(false);
        check(!request.Complete(new("set_name", id, true, "")), "Unrelated results must not complete hearing requests.");
        request.Update(false, 6);
        check(request.Pending == id && request.Complete(new("set_hearing_aid", id, true, "")) && request.Enabled == false,
            "A reported Off must preserve ownership of its pending disable result.");
        request.Update(true, 7);
        request.QueueEdit();
        request.Reset();
        check(!request.Complete(new("set_hearing_aid", id, true, "")) && request.Enabled is null && !request.TakeEdit(),
            "A disconnect or device switch must cancel the old result ownership.");
        request.Update(true, 1);
        check(request.Enabled == true, "A new daemon session may start with a lower revision.");
        id = request.Begin(true);
        request.Cancel();
        check(!request.Complete(new("set_hearing_aid", id, true, "")), "A timed-out request must not alter the switch later.");
        request.Update(null, 2);
        check(request.Enabled is null && !request.QueueEdit(), "Unknown hearing state must not permit an automatic slider apply.");
        Console.WriteLine("Hearing assistance request checks passed.");
    }
}
