using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

internal static class TransparencyChecks
{
    public static void Run(Action<bool, string> check)
    {
        var original = new TransparencySettings { Enabled = false, LeftEq = new float[8], RightEq = new float[8],
            OwnVoice = 0.5f, LeftTone = -0.25f, RightTone = 0.5f };
        check(original.IsValid && original.SameValues(original with { LeftEq = new float[8], RightEq = new float[8] }),
            "Transparency curves compare by value across snapshots.");
        check(!(original with { LeftEq = [] }).IsValid && !(original with { OwnVoice = float.NaN }).IsValid &&
            !(original with { OwnVoice = 1.1f }).IsValid && !(original with { RightEq = [0, 0, 0, 0, 0, 0, 0, 101] }).IsValid,
            "Incomplete or invalid Transparency records must not be writable.");
        var asymmetric = original with { LeftAmplification = -1, RightAmplification = 1 };
        check(TransparencyTuning.Amplification(asymmetric) == -1 && TransparencyTuning.Balance(asymmetric) == 2 &&
            !TransparencyTuning.CanEditCombined(asymmetric),
            "The complete per-ear balance range must survive presentation.");
        var amplified = TransparencyTuning.WithAmplification(original with { LeftAmplification = -0.5f, RightAmplification = 0.25f }, 0.125f);
        check(amplified.IsValid && amplified.LeftAmplification == 0.125f && amplified.RightAmplification == 0.875f &&
            ReferenceEquals(amplified.LeftEq, original.LeftEq), "Native amplification must apply the same delta to both ears.");
        check((amplified with { LeftAmplification = original.LeftAmplification, RightAmplification = original.RightAmplification }).SameValues(original),
            "Combined amplification edits must preserve every unrelated per-ear field.");
        foreach (float gain in new[] { -1f, -0.5f, 0f, 0.5f, 1f })
        foreach (float balance in new[] { -1f, -0.5f, 0f, 0.5f, 1f })
        {
            var adjusted = TransparencyTuning.WithBalance(TransparencyTuning.WithAmplification(original, gain), balance);
            check(adjusted.IsValid && adjusted.LeftAmplification == gain - Math.Min(balance, 0) &&
                adjusted.RightAmplification == gain + Math.Max(balance, 0), "The native gain/balance grid must preserve its exact mapping and range.");
        }
        var positive = TransparencyTuning.WithBalance(original with { LeftAmplification = 0.25f, RightAmplification = 0.5f }, 0.75f);
        var negative = TransparencyTuning.WithBalance(original with { LeftAmplification = 0.5f, RightAmplification = 0.25f }, -0.75f);
        check(positive.LeftAmplification == 0.25f && positive.RightAmplification == 1 &&
            negative.LeftAmplification == 1 && negative.RightAmplification == 0.25f,
            "Positive pan must change only the right gain; negative pan must change only the left gain.");
        var extreme = TransparencyTuning.WithBalance(TransparencyTuning.WithAmplification(original, 1), 1);
        check(extreme.RightAmplification == 2 && extreme.IsValid && !(extreme with { RightAmplification = 2.001f }).IsValid,
            "Captured native extremes require gain up to two while rejecting values above it.");
        var custom = original with { LeftAmplification = 1.25f, RightAmplification = 1.75f };
        check(custom.IsValid && !TransparencyTuning.CanEditCombined(custom) &&
            ReferenceEquals(TransparencyTuning.WithBalance(custom, 0), custom) && ReferenceEquals(TransparencyTuning.WithAmplification(custom, 0), custom),
            "Noncanonical gains must remain untouched rather than being normalized by combined controls.");
        var s = new Snapshot { DeviceId = "first", Connected = true, Transparency = original };
        var edit = new TransparencyEditState();
        edit.Update(s, true);
        var changed = original with { Enabled = true, LeftAmplification = 0.25f };
        edit.Edit(changed);
        s.Transparency = original with { LeftEq = new float[8], RightEq = new float[8] };
        edit.Update(s, true);
        check(edit.Dirty && edit.Draft == changed, "Battery updates must preserve an unsent Transparency edit.");
        s.Transparency = original with { RightTone = -0.75f };
        edit.Update(s, true);
        check(edit.Baseline == s.Transparency && edit.Draft == s.Transparency && !edit.Dirty,
            "A changed current report must replace an obsolete draft and baseline.");
        s.Transparency = original;
        edit.Update(s, true);
        edit.Edit(changed);
        edit.Begin(s, writing: true);
        edit.Update(s, true);
        check(edit.Requesting && edit.Draft == changed, "An old report must not acknowledge a different write.");
        s.TransparencyPending = true;
        edit.Update(s, true);
        check(!edit.Requesting && edit.Draft == changed, "The daemon owns the pending request after acknowledgement.");
        s.TransparencyPending = false;
        s.Transparency = changed with { LeftEq = new float[8], RightEq = new float[8] };
        edit.Update(s, true);
        check(!edit.Dirty && edit.Draft?.SameValues(changed) == true, "A completed write must display confirmed settings.");
        check(edit.Baseline?.SameValues(changed) == true, "Completed writes must use their confirmed record as the next baseline.");
        edit.Begin(s, writing: false);
        edit.Update(s, true);
        check(edit.Requesting, "An unchanged battery snapshot must not acknowledge a read request.");
        s.TransparencyError = "Read failed.";
        edit.Update(s, true);
        check(!edit.Requesting, "A read failure must release the local request latch.");
        edit.Edit(original with { Enabled = true, LeftAmplification = 1, RightAmplification = 2 });
        s.Transparency = null;
        edit.Update(s, true);
        check(edit.Draft is null && edit.Baseline is null && !edit.Dirty && !edit.Requesting,
            "A same-pair connected refresh with no current CT record must discard the cached On draft.");
        s.Transparency = original with { LeftAmplification = 1, RightAmplification = 2 };
        edit.Update(s, true);
        check(edit.Draft?.Enabled == false && edit.Baseline == s.Transparency && !edit.Dirty,
            "A fresh Off record must become the displayed draft after the old baseline was invalidated.");
        edit.Edit(s.Transparency with { Enabled = true });
        s.Transparency = s.Transparency with { RightTone = 0.25f };
        edit.Update(s, true);
        check(edit.Draft?.Enabled == false && edit.Draft == s.Transparency && !edit.Dirty,
            "A changed fresh Off record must replace an unsent obsolete On draft.");
        edit.Edit(changed);
        edit.Begin(s, writing: true);
        s.Transparency = null;
        s.TransparencyPending = true;
        edit.Update(s, true);
        check(edit.Draft is null && edit.Baseline is null && !edit.Dirty && !edit.Requesting,
            "A pending fresh read cannot retain the previous write's draft or baseline.");
        s.TransparencyPending = false;
        edit.Update(s, true);
        check(edit.Draft is null && edit.Baseline is null,
            "A missing report stays uneditable after the pending operation ends.");
        s.Transparency = original;
        edit.Update(s, true);
        edit.Edit(original);
        edit.Update(s, false);
        check(edit.Draft is null && edit.Baseline is null && !edit.Dirty && !edit.Requesting,
            "Disconnect must discard the previous session's draft and baseline.");
        edit.Update(s, true);
        edit.Edit(original);
        s.DeviceId = "second";
        s.Transparency = null;
        edit.Update(s, true);
        check(edit.Draft is null && edit.Baseline is null && !edit.Dirty,
            "A different pair must never inherit Transparency edits or their baseline.");
        check(!DeviceModels.Find("unknown").CustomTransparency && !DeviceModels.Find("A2096").CustomTransparency &&
            DeviceModels.Find("A2083").CustomTransparency && DeviceModels.Find("A3063").CustomTransparency,
            "Only the documented Pro family may expose experimental Custom Transparency discovery.");
        s = new Snapshot { DeviceId = "live", Connected = true, Transparency = original };
        edit.Update(s, true);
        edit.Edit(changed);
        edit.Begin(s, writing: true);
        var latest = changed with { OwnVoice = 0.75f };
        edit.Edit(latest);
        s.TransparencyPending = true;
        edit.Update(s, true);
        check(edit.Sending && edit.Dirty && edit.Draft!.SameValues(latest),
            "Dragging during a write keeps the newest complete Transparency setting.");
        s.TransparencyPending = false;
        s.Transparency = changed;
        edit.Update(s, true);
        check(!edit.Sending && edit.Dirty && edit.Draft!.SameValues(latest) && edit.Baseline!.SameValues(changed),
            "The next live update uses readback as its CAS baseline without reverting the thumb.");
        edit.Begin(s, writing: true);
        edit.Edit(latest with { OwnVoice = 1 });
        s.TransparencyError = "Write failed.";
        edit.Update(s, true);
        check(!edit.Dirty && !edit.Sending && edit.Draft!.SameValues(changed),
            "A failed live write discards queued edits rather than retrying stale complete records.");
        s.TransparencyError = null;
        edit.Update(s, true);
        edit.Edit(latest);
        edit.Begin(s, writing: true);
        edit.Edit(latest with { OwnVoice = 1 });
        s.Transparency = latest;
        s.TransparencyError = "Verification failed after write.";
        edit.Update(s, true);
        check(!edit.Dirty && !edit.Sending && edit.Draft!.SameValues(latest),
            "A matching record with a failed transaction cannot authorize a queued successor.");
        Console.WriteLine("Custom Transparency edit checks passed.");
    }
}
