using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

internal static class CaseInformationChecks
{
    public static void Run(Action<bool, string> check)
    {
        foreach (string model in new[] { "A3063", "A3064", "A3065" })
            check(CaseInformationPresentation.CasePnpProduct(model) == 0x2029,
                "Only the tested Pro 3 catalog family selects the case information profile.");
        foreach (string? model in new string?[] { null, "unknown", "A2084", "A2698", "AirPods Pro 3" })
            check(CaseInformationPresentation.CasePnpProduct(model) is null,
                "Unknown models, display names and other generations cannot inherit case support.");
        check(CaseInformationPresentation.AdvertisedCasePnpProduct(0x2720) == 0x2029 &&
            CaseInformationPresentation.AdvertisedCasePnpProduct(0x1420) is null &&
            CaseInformationPresentation.AdvertisedCasePnpProduct(null) is null,
            "The Pro 3 proximity model must resolve through the catalog without widening support.");
        check(CaseInformationPresentation.ProtocolVersion(0x00010000) == "1.0.0" &&
            CaseInformationPresentation.ProtocolVersion(uint.MaxValue) == "65535.255.255",
            "DULT version fields are major16/minor8/revision8, without Find My version claims.");
        check(CaseInformationPresentation.Raw(0x005a8600) == "0x005A8600" &&
            CaseInformationPresentation.Raw(0x80000000) == "0x80000000",
            "Firmware and unknown unsigned values retain exact hexadecimal representation.");
        check(CaseInformationPresentation.CapabilityKeys(0x3b).SequenceEqual(new[]
            { "NearbyCase.Sound", "NearbyCase.Motion", "NearbyCase.BluetoothIdentity" }),
            "Only standard capability bits receive names; the case's private bits remain unnamed.");
        check(CaseInformationPresentation.UnknownCapabilities(0x8000003b) == 0x80000030 &&
            !CaseInformationPresentation.CapabilityKeys(0x80000030).Any() &&
            CaseInformationPresentation.CapabilityKeys(4).Single() == "NearbyCase.Nfc",
            "Unknown high bits remain unsigned and must not be confused with NFC or another known feature.");

        var state = new CaseInformationPresentation();
        var snapshot = new Snapshot { DeviceId = "first", Model = "A3063", MicState = "idle" };
        var metadata = new CaseMetadata(0x00010000, 0x3b, 0x005a8600);
        check(!state.CanRead, "A case read requires a selected supported pair and ready client.");
        state.Update(snapshot, true);
        check(state.CanRead, "A nearby case can be read while the buds' audio connection is idle or disconnected.");
        int first = state.Begin();
        check(state.Busy && !state.CanRead, "A running read excludes duplicate requests.");
        check(state.Complete(first, new(metadata, CaseInformationError.None)) && state.Metadata == metadata,
            "Only complete success publishes the nearby case's information.");
        check(!state.Update(snapshot, true) && state.Metadata == metadata,
            "An unrelated snapshot does not erase a current result.");
        int old = state.Begin();
        snapshot.DeviceId = "second";
        check(state.Update(snapshot, true) && !state.Busy && state.Metadata is null &&
            !state.Complete(old, new(metadata, CaseInformationError.None)),
            "Changing the selected pair clears the result and rejects the previous read's late completion.");
        int current = state.Begin();
        check(state.Complete(current, new(null, CaseInformationError.Ambiguous)) && state.Metadata is null &&
            CaseInformationPresentation.ErrorKey(state.Error) == "NearbyCase.Ambiguous",
            "Multiple nearby cases produce an explicit ambiguity instead of a substitute result.");
        current = state.Begin();
        state.Complete(current, new(metadata, CaseInformationError.Unreachable));
        check(state.Metadata is null && state.Error == CaseInformationError.Unreachable,
            "Endpoint loss clears metadata even if an erroneous transport result includes old data.");
        current = state.Begin();
        state.Complete(current, new(null, CaseInformationError.None));
        check(state.Error == CaseInformationError.InvalidResponse, "An empty success cannot look like a completed read.");
        current = state.Begin();
        state.Update(snapshot, false);
        check(!state.CanRead && !state.Complete(current, new(metadata, CaseInformationError.None)),
            "Client disconnection cancels the session and rejects a late transport result.");
        foreach (string phase in new[] { "starting", "capturing", "stopping", "recovering", "error", "" })
        {
            snapshot.MicState = "idle";
            state.Update(snapshot, true);
            int generation = state.Begin();
            snapshot.MicState = phase;
            check(state.Update(snapshot, true) && !state.CanRead &&
                !state.Complete(generation, new(metadata, CaseInformationError.None)),
                "A microphone transition cancels case scanning before a reply can update the card.");
        }
        snapshot.MicState = "idle";
        foreach (Action<bool> setFlag in new Action<bool>[]
        {
            value => snapshot.MicCaptureActive = value,
            value => snapshot.MicUplinkActive = value,
            value => snapshot.MicRecording = value,
            value => snapshot.MicManualActive = value
        })
        {
            state.Update(snapshot, true);
            int generation = state.Begin();
            setFlag(true);
            state.Update(snapshot, true);
            check(!state.CanRead && !state.Complete(generation, new(metadata, CaseInformationError.None)),
                "Every microphone activity flag cancels a case read, independent of phase timing.");
            setFlag(false);
        }
        snapshot.Model = "A2084";
        state.Update(snapshot, true);
        check(!state.CanRead && state.Product is null, "Switching to an unsupported model removes case read availability.");
        bool refused = false;
        try { state.Begin(); } catch (InvalidOperationException) { refused = true; }
        check(refused, "An unavailable presentation cannot start transport work.");
        check(CaseInformationPresentation.ErrorKey(CaseInformationError.Cancelled) is null &&
            CaseInformationPresentation.ErrorKey(CaseInformationError.RestoreUncertain) == "NearbyCase.RestoreUncertain" &&
            CaseInformationPresentation.ErrorKey(CaseInformationError.Paired) == "NearbyCase.Paired",
            "Cancellation stays quiet while a paired endpoint or uncertain cleanup remains explicit.");

        foreach (string reason in new[] { "cancel", "pair", "model", "microphone", "daemon" })
        {
            var cancelled = new CaseInformationPresentation();
            var selected = new Snapshot { DeviceId = "first", Model = "A3063", MicState = "idle" };
            cancelled.Update(selected, true);
            int pending = cancelled.Begin();
            switch (reason)
            {
                case "cancel": cancelled.Cancel(); break;
                case "pair": selected.DeviceId = "second"; cancelled.Update(selected, true); break;
                case "model": selected.Model = "A2084"; cancelled.Update(selected, true); break;
                case "microphone": selected.MicCaptureActive = true; cancelled.Update(selected, true); break;
                case "daemon": cancelled.Update(selected, false); break;
            }
            check(!cancelled.Complete(pending, new(null, CaseInformationError.RestoreUncertain)) &&
                cancelled.Metadata is null && !cancelled.CleanupConfirmed &&
                cancelled.Error == CaseInformationError.RestoreUncertain,
                $"Cancelling for {reason} rejects metadata without losing a later cleanup failure.");
            cancelled.Cancel();
            check(!cancelled.Complete(pending, new(metadata, CaseInformationError.None)) &&
                !cancelled.CleanupConfirmed && cancelled.Error == CaseInformationError.RestoreUncertain,
                "Repeated stops and stale successful results cannot acknowledge an earlier cleanup failure.");
            selected.Model = "A3063";
            selected.MicCaptureActive = false;
            cancelled.Update(selected, true);
            int next = cancelled.Begin();
            check(cancelled.Complete(next, new(metadata, CaseInformationError.None)) &&
                cancelled.Metadata == metadata && !cancelled.CleanupConfirmed,
                "A later nearby case read cannot prove the previous case's connection was restored.");
        }
        var clean = new CaseInformationPresentation();
        clean.Update(new Snapshot { DeviceId = "first", Model = "A3063", MicState = "idle" }, true);
        int cleanRead = clean.Begin();
        clean.Cancel();
        check(!clean.Complete(cleanRead, new(null, CaseInformationError.Cancelled)) &&
            clean.CleanupConfirmed && clean.Error == CaseInformationError.None,
            "Clean cancellation does not invent a restoration warning.");
        Console.WriteLine("Nearby case presentation checks passed.");
    }
}
