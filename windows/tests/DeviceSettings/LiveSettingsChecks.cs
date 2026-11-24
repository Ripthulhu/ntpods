using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class LiveSettingsChecks
{
    public static void Run(Action<bool, string> check)
    {
        var live = new LiveSettingEdit<LiveControlValue>();
        LiveControlValue old = new(0), first = new(10), final = new(90);
        check(LiveDeviceChoices.Hold(-1, 0) is null && LiveDeviceChoices.Hold(1, -1) is null &&
            LiveDeviceChoices.Hold(0, 1) == 0x0501 && LiveDeviceChoices.Hold(1, 0) == 0x0105 &&
            LiveDeviceChoices.Hold(0, 0) == 0x0505 && LiveDeviceChoices.Hold(1, 1) == 0x0101,
            "Noise Control is 5 and Assistant is 1; left occupies the second byte and right the first.");
        check(LiveDeviceChoices.HoldIndex(0x01050501, right: false) == 0 &&
            LiveDeviceChoices.HoldIndex(0x01050501, right: true) == 1 &&
            LiveDeviceChoices.HoldIndex(0x05010105, right: false) == 1 &&
            LiveDeviceChoices.HoldIndex(0x05010105, right: true) == 0,
            "Asymmetric current hold actions decode to the correct picker labels without using previous actions.");
        check(LiveDeviceChoices.HoldIndex(null, right: false) == -1 &&
            LiveDeviceChoices.HoldIndex(0x0000, right: true) == -1,
            "Missing and unknown hold actions do not invent a picker selection.");
        check(LiveDeviceChoices.NoiseCycle(0) is null && LiveDeviceChoices.NoiseCycle(1) is null &&
            LiveDeviceChoices.NoiseCycle(3) == 3 && LiveDeviceChoices.NoiseCycle(6) == 6 && LiveDeviceChoices.NoiseCycle(0x13) is null,
            "Noise cycling requires at least two supported modes, never a partial mask.");
        live.Update("pair/session1", true, old, null);
        check(live.Take() is null, "Hydration must never produce a write.");
        live.Edit(first);
        check(live.Take() == first, "A valid user edit is submitted once.");
        live.Edit(new(30)); live.Edit(final);
        live.Update("pair/session1", true, old, first);
        check(live.Take() is null && live.Displayed == final, "In-flight feedback must preserve the latest queued edit.");
        live.Update("pair/session1", true, first, null);
        check(live.Take() == final, "The final queued value must follow the completed request.");
        live.Update("pair/session1", true, final, null);
        check(!live.Busy && live.Take() is null, "The final reply must settle without another write.");
        live.Edit(first); live.Edit(null);
        check(live.Take() is null, "An invalid or incomplete combined edit cancels its queued candidate.");
        live.Edit(first); live.Take(); live.Edit(old);
        live.Update("pair/session1", true, first, null);
        check(live.Take() == old, "Returning to the old value while a request is pending must still send the final value.");
        live.Edit(final);
        live.Update("pair/session2", true, old, null);
        check(!live.Busy && live.Desired is null, "A new session discards pending and queued edits.");
        live.Edit(final); live.Update("other-pair/session2", true, old, null);
        check(live.Take() is null, "Pair changes must never inherit queued edits.");
        live.Edit(final); live.Update("other-pair/session2", false, old, null);
        check(live.Take() is null, "Unavailable controls must discard queued edits.");
        live.Update("other-pair/session2", true, old, null); live.Edit(first); live.Take(); live.Edit(final);
        live.Update("other-pair/session2", true, old, null, "Write failed");
        check(live.Take() is null && !live.Busy, "A failed request discards its queued successor without retrying.");
        live.Edit(first); live.Take(); live.Edit(final);
        live.Update("other-pair/session2", true, new(7), null);
        check(live.Take() is null && !live.Busy, "An unrelated changed report invalidates queued edits.");
        live.Edit(final);
        live.Update("other-pair/session2", true, new(8), null);
        check(live.Take() is null, "An external report during coalescing must invalidate the staged candidate.");
        live.Cancel(); check(live.Take() is null && !live.Busy, "Unloading cancels all local work.");

        var slow = new LiveSettingEdit<LiveControlValue>();
        var recent = new LiveSettingEdit<LiveControlValue>();
        slow.Update("pair", true, old, null, "Repeated error");
        recent.Update("pair", true, old, null);
        slow.Edit(first); slow.Take(100);
        slow.Edit(final);
        slow.Update("pair", true, old, null, "Repeated error");
        recent.Edit(first); recent.Take(10_000);
        check(!slow.Expire(11_099, 11_000) && slow.Requested == first,
            "An unchanged error cannot be mistaken for a correlated rejection before the request bound.");
        check(slow.Expire(11_100, 11_000) && !slow.Busy && slow.Desired is null,
            "An unacknowledged control expires at its own bound and discards its unsent successor.");
        check(!recent.Expire(11_100, 11_000) && recent.Requested == first,
            "Expiring an old control must not cancel a newer control's in-flight request.");
        recent.Edit(final);
        check(recent.Expire(21_000, 11_000) && !recent.Busy,
            "Editing or sending another control cannot extend an existing request's deadline.");
        recent.Edit(first); recent.Take(30_000);
        recent.Update("pair", true, first, null);
        recent.Edit(final);
        check(!recent.Expire(50_000, 11_000) && recent.Desired == final,
            "A completed request's old timeout must not discard a newly staged edit.");

        var eq = new EqualizerEditState();
        var recommended = new Equalizer { Mode = 1, Low = 50, Mid = 50, High = 50 };
        var custom = new Equalizer { Mode = 2, Low = 20, Mid = 60, High = 100 };
        var latest = custom with { Low = 70 };
        var snapshot = new Snapshot { DeviceId = "pair", Connected = true, Eq = recommended };
        eq.Update(snapshot, true, 1);
        check(eq.Take() is null, "An equalizer report is not a user edit.");
        eq.Edit(custom); check(eq.Take() == custom, "Equalizer changes submit complete three-band records.");
        eq.Edit(latest);
        eq.Update(new Snapshot { DeviceId = "pair", Connected = true, Eq = recommended, EqPending = custom }, true, 1);
        check(eq.Displayed == latest && eq.Take() is null, "EQ pending snapshots cannot erase the next slider value.");
        eq.Update(new Snapshot { DeviceId = "pair", Connected = true, Eq = custom }, true, 1);
        check(eq.Take() == latest, "EQ final band values follow the previous acknowledgement.");
        eq.Edit(custom); eq.Update(snapshot, true, 2);
        check(!eq.Dirty && eq.Requested is null, "A new client session invalidates EQ edits even for the same pair.");
        var variable = new PlaybackBitrate { BestEffort = true, Kbps = 256, Variable = true };
        check(variable.IsValidFor("aac_eld"), "Variable bitrate commands retain their valid best-effort wire flag.");
        Console.WriteLine("Live setting queue checks passed.");
    }
}
