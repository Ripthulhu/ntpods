using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

internal static class TransparencyChecks
{
    public static void Run(Action<bool, string> check)
    {
        var original = new TransparencySettings { Enabled = false, LeftTone = -0.25f, RightTone = 0.5f };
        var s = new Snapshot { DeviceId = "first", Connected = true, Transparency = original };
        var edit = new TransparencyEditState();
        edit.Update(s, true);
        var changed = original with { Enabled = true, LeftAmplification = 0.25f };
        edit.Edit(changed);
        edit.Update(s, true);
        check(edit.Dirty && edit.Draft == changed, "Battery updates must preserve an unsent Transparency edit.");
        s.Transparency = original with { RightTone = -0.75f };
        edit.Update(s, true);
        check(edit.Baseline == original && edit.Draft == changed,
            "A report changed by another client must not replace the baseline of an unsent edit.");
        s.Transparency = original;
        edit.Begin(s, writing: true);
        edit.Update(s, true);
        check(edit.Requesting && edit.Draft == changed, "An old report must not acknowledge a different write.");
        s.TransparencyPending = true;
        edit.Update(s, true);
        check(!edit.Requesting && edit.Draft == changed, "The daemon owns the pending request after acknowledgement.");
        s.TransparencyPending = false;
        s.Transparency = changed;
        edit.Update(s, true);
        check(!edit.Dirty && edit.Draft == changed, "A completed write must display confirmed settings.");
        check(edit.Baseline == changed, "Completed writes must use their confirmed record as the next baseline.");
        edit.Begin(s, writing: false);
        edit.Update(s, true);
        check(edit.Requesting, "An unchanged battery snapshot must not acknowledge a read request.");
        s.TransparencyError = "Read failed.";
        edit.Update(s, true);
        check(!edit.Requesting, "A read failure must release the local request latch.");
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
        Console.WriteLine("Custom Transparency edit checks passed.");
    }
}
