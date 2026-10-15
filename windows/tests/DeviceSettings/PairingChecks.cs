using NTPods.WinUI.Services;

internal static class PairingChecks
{
    public static void Run(Action<bool, string> check)
    {
        var state = new PairingDiscoveryState();
        int first = state.Begin();
        state.Update(first, "renamed", "Work headphones", false, true, true);
        var renamed = state.Find("renamed");
        check(renamed is not null, "Manual pairing must include renamed devices without guessing a model.");
        state.Update(first, "renamed", "AirPods Work", false, true, true);
        check(ReferenceEquals(renamed, state.Find("renamed")) && renamed?.Name == "AirPods Work",
            "A name update must preserve the selected endpoint object.");
        state.Update(first, "already", "AirPods", true, true, true);
        state.Update(first, "unavailable", "AirPods", false, false, true);
        state.Update(first, "gone", "AirPods", false, true, false);
        check(state.Candidates.Count == 1, "Only present, unpaired, pairable Windows endpoints may be offered.");
        state.Remove(first, "renamed");
        check(state.Find("renamed") is null, "A removed endpoint must immediately cease to be selectable.");
        state.Update(first, "changing", "AirPods", false, true, true);
        state.Update(first, "changing", "AirPods", true, false, true);
        check(state.Candidates.Count == 0, "Pairing elsewhere must remove an existing candidate.");

        state.Stop();
        state.Update(first, "late", "AirPods", false, true, true);
        check(!state.Scanning && state.Candidates.Count == 0, "Stopped or closed discovery must reject queued callbacks.");
        int second = state.Begin();
        state.Update(first, "stale", "AirPods", false, true, true);
        state.Update(second, "current", "AirPods", false, true, true);
        state.Remove(first, "current");
        check(state.Candidates.Count == 1 && state.Find("current") is not null,
            "Callbacks from a previous search cannot add or remove candidates in a retry.");

        var prompts = new PairingInvitationPolicy();
        check(prompts.Next(new[] { new PairingCandidate("renamed", "Work headphones") }) is null,
            "Automatic invitation must not claim an unrelated or renamed device is AirPods.");
        check(prompts.Next(state.Candidates)?.Id == "current", "An unpaired AirPods name may suggest an invitation.");
        check(prompts.Next(state.Candidates) is null, "Repeated advertisements must not repeat a dismissed invitation.");
        state.Remove(second, "current");
        state.Update(second, "current", "AIRPODS renamed", false, true, true);
        check(prompts.Next(state.Candidates) is null, "Removal, reappearance and renaming cannot evade session suppression.");
        check(new PairingInvitationPolicy().Next(state.Candidates)?.Id == "current",
            "Only a new app session resets automatic prompt suppression.");
        var lidPrompts = new PairingInvitationPolicy();
        lidPrompts.Suppress(state.Candidates);
        check(lidPrompts.Next(state.Candidates) is null,
            "A Classic device already shown inside a lid invitation must not reopen a dismissed card.");
        CheckNearbyInvitations(check);
        CheckLidState(check);
        state.Close();
        check(!state.Scanning && state.Candidates.Count == 0,
            "Closing discovery must invalidate pending callbacks and release selectable endpoints.");
        state.Begin();
        check(state.Candidates.Count == 0, "A new scan must not retain stale pairing choices.");
        check(PairingConfirmation.Allowed("selected", "selected", true, false),
            "Explicit selection authorizes that endpoint's ConfirmOnly ceremony.");
        check(!PairingConfirmation.Allowed("selected", "other", true, false) &&
              !PairingConfirmation.Allowed("selected", "selected", false, false) &&
              !PairingConfirmation.Allowed("selected", "selected", true, true),
            "Other endpoints, PIN ceremonies and cancelled requests must never be accepted.");
        check(PairingInstructions.ForName("AirPods Pro 3") == "Pairing.DoubleTapCase" &&
              PairingInstructions.ForName("AIRPODS 4") == "Pairing.DoubleTapCase" &&
              PairingInstructions.ForName("AirPods 5 (wireless charging case)") == "Pairing.DoubleTapCase" &&
              PairingInstructions.ForName("AirPods Max") == "Pairing.HoldNoiseControl" &&
              PairingInstructions.ForName("My AirPods") == "Pairing.PrepareCase" &&
              PairingInstructions.ForName("AirPods Pro") == "Pairing.PrepareCase",
            "Pairing guidance must cover both case controls when the model is unknown.");
        CheckRetries(check).GetAwaiter().GetResult();
        CheckAutomaticSelection(check);
        CheckCandidateChanges(check);
        Console.WriteLine("Pairing discovery and invitation checks passed.");
    }

    private static void CheckNearbyInvitations(Action<bool, string> check)
    {
        check(PairingInvitationPolicy.NearbyName(0x2720) == DeviceModels.Find("A3063").Name,
            "A lid invitation uses the catalog name for its advertised model.");
        check(PairingInvitationPolicy.NearbyName(0x0e20) == DeviceModels.Find("A2084").Name,
            "Each nearby case uses its own advertised model name.");
        check(PairingInvitationPolicy.NearbyName(0xffff) is null && PairingInvitationPolicy.NearbyName(0x0a20) is null,
            "Unknown models and headphones must not become lid-open invitations.");
        foreach (ushort model in new ushort[] { 0x0220, 0x0f20, 0x1320, 0x1920, 0x1b20, 0x3620, 0x3020, 0x1420, 0x2420 })
            check(PairingInvitationPolicy.NearbyName(model) is { Length: > 0 },
                "Every supported earbud advertisement must resolve an existing catalog name.");
    }

    private static void CheckLidState(Action<bool, string> check)
    {
        var lid = new PairingLidState();
        check(!lid.Close(0x2720), "Lid reports cannot dismiss a Classic or reconnect invitation.");
        lid.Show(0x2720);
        check(!lid.Close(0x0e20), "A different model's closed lid cannot dismiss the current invitation.");
        lid.QueueOpen(0x2720);
        check(lid.Dismissed() is null, "Repeated open reports must not reopen a normally dismissed card.");

        lid.Show(0x2720);
        check(lid.Close(0x2720), "Closing the matching lid must dismiss its invitation.");
        lid.QueueOpen(0x2720);
        check(lid.Dismissed() == 0x2720,
            "A quick reopen during dismissal must survive until the animation finishes.");
        lid.Show(0x2720);
        check(lid.Close(0x2720) && lid.Dismissed() is null,
            "A close without a later open must leave the invitation dismissed.");
        lid.Show(0x2720);
        check(lid.Close(0x2720), "Reopening immediately must restore matching close handling without a cooldown.");
        lid.QueueOpen(0x0e20);
        check(!lid.Close(0x0e20) && lid.Dismissed() is null,
            "Closing a queued case must remove its pending invitation without dismissing another source.");

        lid.Show(0x2720);
        lid.Close(0x2720);
        lid.QueueOpen(0x2720);
        lid.Clear();
        check(lid.Dismissed() is null && !lid.Close(0x2720),
            "Switching flow or stopping pairing must clear both lid tracking and queued invitations.");

    }

    private static void CheckAutomaticSelection(Action<bool, string> check)
    {
        check(new PairingCandidate("headset", "AirPods", 4, 1).IsAirPods &&
              new PairingCandidate("headphones", "AIRPODS Pro", 4, 6).IsAirPods &&
              new PairingCandidate("missing-class", "AirPods").IsAirPods &&
              new PairingCandidate("unknown-class", "AirPods", 0, 0).IsAirPods,
            "Headphones and missing class metadata must allow named AirPods through discovery.");
        check(!new PairingCandidate("tv", "AirPods TV", 4, 15).IsAirPods &&
              !new PairingCandidate("computer", "AirPods PC", 1, 7).IsAirPods &&
              !new PairingCandidate("speaker", "AirPods speaker", 4, 5).IsAirPods &&
              !new PairingCandidate("other", "Sony headphones", 4, 6).IsAirPods,
            "A name and any available headphone class must agree; unrelated devices must not be chosen.");

        var state = new PairingDiscoveryState();
        int generation = state.Begin();
        state.Update(generation, "first", "AirPods", false, true, true, 4, 6);
        check(state.AutomaticCandidate(0, out bool multiple) is null && !multiple &&
              state.AutomaticCandidate(1999, out _) is null && state.AutomaticCandidate(2000, out _) == "first",
            "A sole AirPods endpoint must settle for two seconds without requiring EnumerationCompleted.");
        state.Update(generation, "first", "AirPods", false, true, true, 4, 6);
        check(state.AutomaticCandidate(3100, out _) == "first",
            "Unchanged discovery updates must not postpone an otherwise stable endpoint.");

        state.Update(generation, "second", "AirPods", false, true, true);
        check(state.AutomaticCandidate(4000, out multiple) is null && multiple &&
              state.AutomaticCandidate(10000, out multiple) is null && multiple,
            "Multiple AirPods endpoints must wait rather than choose the first or closest one.");
        state.Remove(generation, "second");
        check(state.AutomaticCandidate(10000, out multiple) is null && !multiple &&
              state.AutomaticCandidate(11999, out _) is null && state.AutomaticCandidate(12000, out _) == "first",
            "Resolving ambiguity must start a fresh settling interval.");
        state.Update(generation, "second", "AirPods", false, true, true);
        state.Remove(generation, "second");
        check(state.AutomaticCandidate(12001, out _) is null,
            "A competing endpoint appearing between polls must still reset selection.");
        state.Remove(generation, "first");
        check(state.AutomaticCandidate(13000, out _) is null, "Removed endpoints must never survive settling.");
        state.Update(generation, "tv", "AirPods TV", false, true, true, 4, 15);
        state.Update(generation, "renamed", "Work headphones", false, true, true, 4, 6);
        check(state.Candidates.Count == 2 && state.AutomaticCandidate(14000, out multiple) is null && !multiple,
            "Generic manual discovery stays available while automatic pairing ignores other devices.");
        state.Update(generation, "first", "AirPods", false, true, true);
        state.AutomaticCandidate(15000, out _);
        state.Stop();
        check(state.AutomaticCandidate(17000, out _) is null,
            "Stopping or cancelling discovery must invalidate its pending automatic choice.");
        int retry = state.Begin();
        state.Update(generation, "stale", "AirPods", false, true, true);
        state.Update(retry, "current", "AirPods", false, true, true);
        check(state.AutomaticCandidate(19000, out _) is null && state.Find("stale") is null &&
              state.AutomaticCandidate(21000, out _) == "current",
            "Restarted discovery must reject old callbacks and settle its own sole candidate.");
        state.Close();
        check(state.AutomaticCandidate(23000, out _) is null,
            "Closing discovery must clear any remaining automatic pairing target.");
    }

    private static void CheckCandidateChanges(Action<bool, string> check)
    {
        var state = new PairingDiscoveryState();
        int generation = state.Begin();
        state.Update(generation, "pods", "AirPods", false, true, true, 4, 6);
        check(state.AutomaticCandidate(0, out _) is null, "AirPods must begin their settling interval.");
        for (int now = 100; now <= 2000; now += 100)
        {
            state.Update(generation, "tv", "TV in living room", false, true, true, 4, 15);
            state.Update(generation, "tv", "AirPods TV", false, true, true, 4, 15);
            state.Remove(generation, "tv");
            check(state.AutomaticCandidate(now, out bool multiple) == (now == 2000 ? "pods" : null) && !multiple,
                "Unrelated discovery changes must not restart the sole AirPods endpoint's settling interval.");
        }
        state.Update(generation, "other", "AirPods TV", false, true, true, 4, 15);
        state.Update(generation, "other", "AirPods", false, true, true, 4, 6);
        check(state.AutomaticCandidate(2100, out bool ambiguous) is null && ambiguous,
            "An endpoint becoming AirPods must invalidate selection and expose the ambiguity.");
        state.Update(generation, "other", "AirPods TV", false, true, true, 4, 15);
        check(state.AutomaticCandidate(2200, out _) is null && state.AutomaticCandidate(4200, out _) == "pods",
            "An endpoint ceasing to be AirPods must start a fresh settling interval for the remaining pair.");
        state.Update(generation, "other", "AirPods", false, true, true, 4, 6);
        state.Update(generation, "other", "AirPods TV", false, true, true, 4, 15);
        check(state.AutomaticCandidate(4201, out _) is null,
            "A competing endpoint recognized and rejected between polls must still reset selection.");
    }

    private static async Task CheckRetries(Action<bool, string> check)
    {
        int attempts = 0;
        await PairingRetry.WaitAsync(_ => Task.FromResult(++attempts == 2), CancellationToken.None);
        check(attempts == 2, "Not-ready pairing must wait and retry until the device accepts it.");

        using var cancelled = new CancellationTokenSource();
        cancelled.Cancel();
        attempts = 0;
        try
        {
            await PairingRetry.WaitAsync(_ => { ++attempts; return Task.FromResult(true); }, cancelled.Token);
            check(false, "Cancelled pairing must not start.");
        }
        catch (OperationCanceledException) { check(attempts == 0, "Cancellation must precede the first pairing call."); }

        using var timeout = new CancellationTokenSource(TimeSpan.FromMilliseconds(50));
        attempts = 0;
        try
        {
            await PairingRetry.WaitAsync(_ => { ++attempts; return Task.FromResult(false); }, timeout.Token);
            check(false, "Pairing must respect its overall timeout while waiting for the gesture.");
        }
        catch (OperationCanceledException) { check(attempts == 1, "Cancelling the retry wait must prevent another pairing call."); }

        using var closing = new CancellationTokenSource();
        try
        {
            await PairingRetry.WaitAsync(_ =>
            {
                closing.Cancel();
                return Task.FromResult(true);
            }, closing.Token);
            check(false, "A completion after cancellation must not hand off or revive the prompt.");
        }
        catch (OperationCanceledException) { }

        attempts = 0;
        try
        {
            await PairingRetry.WaitAsync(_ =>
            {
                ++attempts;
                throw new UnauthorizedAccessException();
            }, CancellationToken.None);
            check(false, "An access failure must leave the retry loop.");
        }
        catch (UnauthorizedAccessException) { check(attempts == 1, "Permission failures must not retry."); }
    }
}
