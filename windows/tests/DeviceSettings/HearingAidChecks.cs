using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class HearingAidChecks
{
    public static void Run(Action<bool, string> check)
    {
        CheckDraft(check);
        CheckEnableRecovery(check);
        CheckMediaRequest(check);
        CheckModeRequest(check);
        CheckRefreshOwnership(check);
        CheckDraftOrdering(check);
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

    private static void CheckMediaRequest(Action<bool, string> check)
    {
        var request = new HearingAidRequest("set_media_assist");
        request.Update(false, 1);
        ulong id = request.Begin(true);
        check(!request.Complete(new("set_hearing_aid", id, true, "")) && request.Pending == id,
            "A hearing-aid result must not complete a media-assist request with the same identifier.");
        check(request.Complete(new("set_media_assist", id, true, "")) && request.Enabled == false && request.EnableUnconfirmed,
            "Media-assist acknowledgment must not manufacture an On report or clear its recovery action.");
        request.Update(true, 2);
        check(request.Enabled == true && !request.EnableUnconfirmed,
            "A fresh media-assist report confirms the switch.");
        request.Begin(false);
        request.Reset();
        check(!request.CanDisable && request.Pending is null && request.Enabled is null,
            "Media state and pending ownership must not survive a device or daemon-session reset.");
    }

    private static void CheckDraftOrdering(Action<bool, string> check)
    {
        var request = new HearingAidRequest("set_hearing_aid_mode");
        request.Update(true, 1);
        check(request.QueueEdit(requireApply: true) && request.ManualEditQueued && !request.TakeEdit() && request.EditQueued,
            "An audiogram edit must remain queued until Apply, even after the tuning debounce expires.");
        check(request.QueueEdit() && request.ManualEditQueued && !request.TakeEdit(),
            "A later slider gesture must not automatically send an uncommitted audiogram.");
        check(request.TakeEdit(explicitApply: true) && !request.ManualEditQueued && !request.EditQueued,
            "Apply must take a complete audiogram draft once and clear its manual queue.");
        check(request.QueueEdit() && request.TakeEdit(),
            "A tuning-only gesture retains automatic debouncing after a manual draft has finished.");
        request.QueueEdit(requireApply: true);
        check(!request.Update(false, 1) && request.EditQueued,
            "An old mode report must not discard a newer audiogram draft.");
        check(request.Update(false, 2) && !request.EditQueued && !request.ManualEditQueued,
            "A fresh Off report must invalidate the card draft and its queued Apply.");
        request.Update(true, 3);
        check(!request.TakeEdit(explicitApply: true),
            "Re-enabling the mode must not revive edits discarded by Off.");
        request.QueueEdit(requireApply: true);
        check(request.Update(null, 4) && !request.EditQueued,
            "A fresh unknown mode must invalidate an unsent personal draft.");

        var original = new HearingAidSettings { LeftEq = new float[8], RightEq = new float[8] };
        var snapshot = new Snapshot { DeviceId = "pair", HearingAidEditable = true, HearingAidSettings = original };
        var edit = new HearingAidEditState();
        edit.Update(snapshot, true);
        edit.EditTone(0.75f);
        request.Update(true, 5);
        request.QueueEdit();
        if (request.Update(false, 6)) edit.Reset();
        edit.Update(snapshot, true);
        request.Update(true, 7);
        check(!edit.Dirty && edit.Draft!.SameValues(original) && !request.TakeEdit(),
            "Turning the mode off and on must restore actual settings instead of resurrecting an unsent tone draft.");
        request.QueueEdit(requireApply: true);
        check(!request.Update(false, 8, verifying: true) && !request.EditQueued,
            "Temporary verification state cancels scheduling without discarding the preserved readback.");

        var media = new HearingAidRequest("set_media_assist");
        media.Update(false, 1);
        ulong old = media.Begin(true);
        ulong off = request.Begin(false);
        check(request.Complete(new("set_hearing_aid_mode", off, false, "Wait for the other mode.")) &&
            media.Pending == old && media.Complete(new("set_media_assist", old, true, "")),
            "A rejected cross-mode Off must retain the original enable's confirmation ownership.");
        old = media.Begin(true);
        off = request.Begin(false);
        check(media.Complete(new("set_media_assist", old, false, "")) && media.Pending is null &&
            media.EnableUnconfirmed && media.CanDisable && request.Pending == off,
            "A superseded enable completes only its matching wait while retaining the newer Off and recovery action.");
        request.Complete(new("set_hearing_aid_mode", off, true, ""));
        old = media.Begin(true);
        off = media.Begin(false);
        check(!media.Complete(new("set_media_assist", old, false, "")) && media.Pending == off &&
            media.Complete(new("set_media_assist", off, true, "")),
            "Emergency Off must supersede its own pending enable without accepting its late result.");
    }

    private static void CheckModeRequest(Action<bool, string> check)
    {
        var request = new HearingAidRequest("set_hearing_aid_mode");
        check(!request.CanEnable(true), "Enrollment alone must not enable an unknown hearing mode.");
        request.Update(false, 1);
        check(!request.CanEnable(null) && !request.CanEnable(false) && request.CanEnable(true),
            "A confirmed fitted profile and known mode may enable without an editable audiogram.");
        ulong id = request.Begin(true);
        check(!request.CanEnable(true) && request.CanDisable,
            "An unconfirmed mode enable must keep Off available and block another enable.");
        check(!request.Complete(new("set_hearing_aid", id, true, "")) && request.Pending == id,
            "A tuning result must not complete a mode request.");
        request.Update(null, 2, verifying: true);
        check(request.Pending == id && request.Enabled is null && request.EnableMayHaveApplied,
            "A missing mode report during the backend refresh must retain request ownership and recovery.");
        request.Update(false, 3, verifying: true);
        check(request.Pending == id && request.EnableMayHaveApplied,
            "A temporary Off report during verification must not cancel the pending mode request.");
        request.Complete(new("set_hearing_aid_mode", id, true, ""));
        request.Update(true, 4);
        id = request.Begin(true, "set_hearing_aid");
        check(!request.Complete(new("set_hearing_aid_mode", id, true, "")) &&
            request.Complete(new("set_hearing_aid", id, true, "")),
            "A tuning request must retain its own command-result ownership.");
        id = request.Begin(false);
        request.Update(false, 5, verifying: true);
        check(request.Pending == id && request.EnableUnconfirmed &&
              !request.Complete(new("set_media_assist", id, true, "")),
            "An interim Off report or unrelated result must not finish the mode request.");
        check(request.Complete(new("set_hearing_aid_mode", id, true, "")),
            "The next mode request must not inherit the preceding tuning command.");
        request.Update(false, 5);
        check(!request.EnableMayHaveApplied && !request.EnableUnconfirmed && request.Pending is null,
            "Verified Off must clear recovery even when its final report repeats the interim revision.");
    }

    private static void CheckRefreshOwnership(Action<bool, string> check)
    {
        var mode = new HearingAidRequest("set_hearing_aid_mode");
        var media = new HearingAidRequest("set_media_assist");
        var snapshot = new Snapshot { DeviceId = "pair", Connected = false, HearingAidPending = true };
        mode.Begin(true);
        check(HearingAidRequest.RetainWhileRefreshing(snapshot, "pair", mode, media),
            "A same-device soft refresh must keep the mode request while the link is briefly down.");
        snapshot.DeviceId = "other";
        check(!HearingAidRequest.RetainWhileRefreshing(snapshot, "pair", mode, media),
            "Selecting another pair must release the old pending request.");
        snapshot.DeviceId = "pair";
        snapshot.HearingAidPending = false;
        check(!HearingAidRequest.RetainWhileRefreshing(snapshot, "pair", mode, media),
            "A real disconnect without backend verification must not retain the request.");
        mode.Cancel();
        media.Begin(true);
        snapshot.HearingAidPending = true;
        check(HearingAidRequest.RetainWhileRefreshing(snapshot, "pair", mode, media) &&
              !HearingAidRequest.RetainWhileRefreshing(snapshot, "", mode, media),
            "Media-assist verification follows the same selected-pair rule without claiming an empty identity.");
        media.Cancel();
        check(!HearingAidRequest.RetainWhileRefreshing(snapshot, "pair", mode, media),
            "A completed or timed-out local request must not prolong the refresh state.");
    }

    private static void CheckEnableRecovery(Action<bool, string> check)
    {
        var request = new HearingAidRequest();
        check(!request.CanDisable && !request.EnableUnconfirmed,
            "An initially unknown state must not invent a previous enable attempt.");
        request.Update(false, 1);
        ulong enable = request.Begin(true);
        check(request.CanDisable && request.EnableUnconfirmed && request.Enabled == false,
            "An enable in progress must remain cancellable without claiming a confirmed On report.");
        request.Complete(new("set_hearing_aid", enable, false, "Settings write failed; cleanup failed."));
        check(request.CanDisable && request.EnableUnconfirmed && request.Pending is null,
            "A failed partial enable must retain an Off action even when no settings are available.");
        request.Update(false, 1);
        check(request.CanDisable, "A cached Off predating the request must not hide the recovery action.");
        request.Update(null, 2);
        check(request.Enabled is null && request.EnableUnconfirmed && request.CanDisable,
            "An unknown report after an enable must preserve the recovery action.");
        ulong disable = request.Begin(false);
        check(!request.Requested && request.CanDisable,
            "Recovery must issue an Off request without requiring a confirmed On state or audiogram.");
        request.Complete(new("set_hearing_aid", disable, false, "Disable was not confirmed."));
        check(request.CanDisable && request.EnableUnconfirmed, "A failed Off must remain retryable.");
        disable = request.Begin(false);
        request.Complete(new("set_hearing_aid", disable, true, ""));
        check(request.CanDisable && request.EnableUnconfirmed,
            "A command acknowledgment alone must not claim that amplification has stopped.");
        request.Update(false, 3);
        check(!request.CanDisable && !request.EnableUnconfirmed && !request.EnableMayHaveApplied,
            "A fresh confirmed Off must clear the uncertain enable latch.");
        request.Begin(true);
        request.Cancel();
        check(request.CanDisable && request.EnableUnconfirmed, "An enable timeout must retain the Off action.");
        request.Reset();
        check(!request.CanDisable && !request.EnableUnconfirmed && request.Enabled is null,
            "Disconnect, device and daemon-session resets must discard recovery ownership.");
    }

    private static void CheckDraft(Action<bool, string> check)
    {
        var edit = new HearingAidEditState();
        check(!edit.CanApply && !edit.EditTone(0), "Unknown settings must not manufacture an editable audiogram.");
        var original = new HearingAidSettings {
            LeftEq = [5, 10, 15, 20, 25, 30, 35, 40], RightEq = [10, 15, 20, 25, 30, 35, 40, 45],
            LeftAmplification = -0.25f, RightAmplification = 0.5f, LeftTone = -0.5f, RightTone = 0.25f,
            LeftConversationBoost = true, RightConversationBoost = false,
            LeftAmbientNoiseReduction = 0.25f, RightAmbientNoiseReduction = 0.75f, OwnVoice = 0.5f
        };
        foreach (float amplification in new[] { -1.5f, -1.25f, 1.25f, 1.5f })
            check((original with { LeftAmplification = amplification, RightAmplification = -amplification }).IsValid,
                "Stored Apple amplification, including balanced values beyond one, must remain readable.");
        foreach (float amplification in new[] { float.NaN, float.PositiveInfinity, float.NegativeInfinity, -1.5001f, 1.5001f })
        {
            check(!(original with { LeftAmplification = amplification }).IsValid &&
                !(original with { RightAmplification = amplification }).IsValid,
                "Amplification outside the supported range must fail validation for either ear.");
        }
        foreach (float tone in new[] { -1.0001f, 1.0001f })
            check(!(original with { LeftTone = tone }).IsValid && !(original with { RightTone = tone }).IsValid,
                "The wider amplification range must not widen tone validation.");
        var balancedRead = original with { LeftAmplification = 0.75f, RightAmplification = 1.25f };
        edit.Update(new Snapshot { DeviceId = "balanced", HearingAidSettings = balancedRead,
            HearingAidEditable = true }, true);
        check(edit.Draft!.SameValues(balancedRead) && edit.Editable && !edit.Dirty &&
              edit.Amplification == 1 && edit.Balance == 0.5f && edit.EditTone(0) &&
              edit.Draft.SameValues(balancedRead with { LeftTone = 0, RightTone = 0 }),
            "Apple's balanced gains above one must remain editable without changing their levels.");
        foreach (var gains in new[] { (1.5f, 1.5f), (-1.5f, 1.5f), (-1.5f, -1.5f) })
        {
            var uncommon = original with { LeftAmplification = gains.Item1, RightAmplification = gains.Item2 };
            edit.Update(new Snapshot { DeviceId = "uncommon", HearingAidSettings = uncommon,
                HearingAidEditable = true }, true);
            check(edit.Editable && !edit.Dirty && edit.Draft!.SameValues(uncommon) &&
                edit.Amplification is >= 0 and <= 1 && edit.Balance is >= -1 and <= 1 &&
                edit.EditTone(0) && edit.Draft.SameValues(uncommon with { LeftTone = 0, RightTone = 0 }),
                "Combined slider limits must not change stored gains on read or an unrelated tone edit.");
            check(edit.EditAmplification(0.5f) && edit.Draft!.LeftAmplification == -edit.Balance * 0.5f &&
                edit.Draft.RightAmplification == edit.Balance * 0.5f && edit.Baseline!.SameValues(uncommon),
                "An explicit gain edit uses the displayed balance and preserves the original expected baseline.");
        }
        var snapshot = new Snapshot { DeviceId = "first", HearingAidSettings = original, HearingAidEditable = true };
        check(edit.Update(snapshot, true) && edit.Draft!.SameValues(original) && !edit.Dirty,
            "Reading settings populates the draft without queuing a write.");
        check(!ReferenceEquals(edit.Draft!.LeftEq, original.LeftEq), "Draft curves must not mutate device readback.");
        check(edit.Amplification == 0.5625f && edit.Balance == 0.75f,
            "The combined controls must be derived from the actual per-ear amplification.");
        check(edit.EditTone(0.75f) && edit.Draft!.SameValues(original with { LeftTone = 0.75f, RightTone = 0.75f }),
            "Changing tone must preserve asymmetric amplification, boosts, noise reduction, own voice and both curves.");
        check(edit.Baseline!.SameValues(original), "Editing a draft must preserve the caller's exact expected baseline.");
        edit.Baseline!.LeftEq[0] = 90;
        check(edit.Baseline!.SameValues(original), "A command's expected settings must not expose mutable baseline arrays.");
        check(!edit.Update(snapshot, true) && edit.Draft!.LeftTone == 0.75f,
            "An unrelated snapshot must not discard an unsent edit.");
        var request = new HearingAidRequest();
        request.Update(true, 1);
        check(request.QueueEdit(), "An editable confirmed setting can queue its draft.");
        snapshot.HearingAidEditable = false;
        if (edit.Update(snapshot, true)) request.CancelEdit();
        check(edit.Draft!.SameValues(original) && !edit.Dirty && !edit.CanApply && !request.TakeEdit(),
            "Losing editability must discard the queued draft but retain the actual read-only values.");
        check(!edit.EditTone(1) && !edit.EditAmplification(1) && !edit.EditBalance(1) &&
            !edit.EditConversationBoost(false) && !edit.EditCurve(new double[8], new double[8]) && request.CanDisable,
            "Read-only settings must block every profile edit while preserving the Off action.");
        var mode = new HearingAidRequest("set_hearing_aid_mode");
        mode.Update(false, 1);
        check(mode.CanEnable(true) && !edit.CanApply && edit.Draft!.SameValues(original),
            "A fitted read-only profile can switch on without making its audiogram editable or changing any values.");
        check(!edit.Update(snapshot, true) && edit.Draft!.SameValues(original),
            "Repeated read-only snapshots must not recreate a discarded edit.");
        snapshot.HearingAidEditable = true;
        check(edit.Update(snapshot, true) && edit.CanApply && !edit.Dirty && edit.Draft!.SameValues(original),
            "A later editable read starts from device values without restoring discarded edits.");
        edit.Reset();
        edit.Update(snapshot, true);
        check(edit.EditConversationBoost(false) && edit.Draft!.SameValues(original with { LeftConversationBoost = false }),
            "Changing conversation boost must preserve asymmetric tone and every unrelated setting.");
        edit.Reset();
        edit.Update(snapshot, true);
        check(edit.EditAmplification(0.5f) && edit.Draft!.SameValues(original with { LeftAmplification = -0.375f, RightAmplification = 0.375f }),
            "Amplification edits must preserve balance and the complete audiogram and tuning data.");
        check(edit.EditBalance(0.25f) && edit.Draft!.LeftAmplification == -0.125f && edit.Draft.RightAmplification == 0.125f,
            "Balance adds and subtracts half its value while preserving mean amplification.");
        var balanced = original with { LeftAmplification = -0.5f, RightAmplification = 0.5f };
        edit.Update(new Snapshot { DeviceId = snapshot.DeviceId, HearingAidSettings = balanced, HearingAidEditable = true }, true);
        check(edit.EditAmplification(1) && edit.Amplification == 1 && edit.Balance == 1 &&
            edit.Draft!.LeftAmplification == 0.5f && edit.Draft.RightAmplification == 1.5f,
            "Maximum amplification must retain balance and allow Apple's full per-ear gain range.");
        check(edit.EditAmplification(0.5f) && edit.Draft!.SameValues(balanced),
            "Returning amplification to its original position must restore both ear levels.");
        check(edit.EditAmplification(0) && edit.Draft!.LeftAmplification == -1.5f &&
            edit.Draft.RightAmplification == -0.5f && edit.Balance == 1,
            "Minimum amplification must retain balance at Apple's negative gain boundary.");
        var amplified = original with { LeftAmplification = 0.5f, RightAmplification = 0.5f };
        edit.Update(new Snapshot { DeviceId = snapshot.DeviceId, HearingAidSettings = amplified, HearingAidEditable = true }, true);
        check(edit.EditBalance(1) && edit.Amplification == 0.75f && edit.Balance == 1 &&
            edit.Draft!.LeftAmplification == 0 && edit.Draft.RightAmplification == 1,
            "Maximum balance must retain the requested amplification for the rest of the drag.");
        check(edit.EditBalance(0) && edit.Draft!.SameValues(amplified),
            "Returning balance to center must restore both ear levels.");
        edit.Update(snapshot, true);
        check(edit.Amplification == 0.5625f && edit.Balance == 0.75f,
            "A fresh device read must replace the requested slider values with its actual ear levels.");
        foreach (double invalid in new[] { double.NaN, double.PositiveInfinity, -1d, 91d })
        {
            double[] left = original.LeftEq.Select(v => (double)v).ToArray();
            left[3] = invalid;
            check(!edit.EditCurve(left, original.RightEq.Select(v => (double)v).ToArray()) && !edit.CanApply && !edit.EditTone(0),
                "Empty, nonfinite or out-of-range thresholds must block writes, including subsequent scalar edits.");
        }
        check(!edit.EditCurve([1, 2], new double[8]) && !edit.CanApply, "A partial audiogram must not be padded with zeroes.");
        double[] corrected = original.LeftEq.Select(v => (double)v).ToArray();
        corrected[3] = 22;
        check(edit.EditCurve(corrected, original.RightEq.Select(v => (double)v).ToArray()) && edit.CanApply &&
            edit.Draft!.LeftEq[3] == 22 && original.LeftEq[3] == 20 && edit.Draft.LeftTone == original.LeftTone,
            "Correcting all thresholds releases validation and preserves unrelated tuning and original readback.");
        snapshot.HearingAidSettings = null;
        check(edit.Update(snapshot, true) && edit.Draft is null && !edit.Dirty && !edit.CanApply,
            "Unknown or failed readback must clear a previously editable draft.");
        snapshot.HearingAidSettings = original with { RightEq = [1, 2] };
        edit.Update(snapshot, true);
        check(edit.Draft is null, "Partial settings must never become a baseline.");
        snapshot.HearingAidSettings = original;
        snapshot.HearingAidError = "Read failed.";
        edit.Update(snapshot, true);
        check(edit.Draft is null, "A read error must invalidate even a retained old settings value.");
        snapshot.HearingAidError = null;
        edit.Update(snapshot, true);
        edit.EditTone(1);
        edit.Update(snapshot, false);
        check(edit.Draft is null && !edit.Dirty, "Disconnecting must discard pending personal settings.");
        edit.Update(snapshot, true);
        edit.EditTone(1);
        edit.Update(new Snapshot { DeviceId = "second" }, true);
        check(edit.Draft is null && !edit.Dirty, "A different device must not inherit the previous audiogram.");
        edit.Update(snapshot, true);
        edit.Reset();
        check(!edit.CanApply && !edit.Editable && edit.Draft is null && edit.Baseline is null && !edit.Dirty &&
            edit.Amplification == 0.5f && edit.Balance == 0,
            "A daemon-session reset must discard the prior baseline.");
        edit.Update(new Snapshot { DeviceId = "legacy", HearingAidSettings = original }, true);
        check(edit.Draft!.SameValues(original) && !edit.Editable && !edit.CanApply,
            "Older snapshots can display reported settings but cannot authorize editing.");
    }
}
