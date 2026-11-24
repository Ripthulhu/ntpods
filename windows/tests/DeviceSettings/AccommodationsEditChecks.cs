using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class AccommodationsEditChecks
{
    public static void Run(Action<bool, string> check)
    {
        var state = new HeadphoneAccommodationsState { Session = 17, Revision = 4, Phone = true, Media = false,
            Editable = true, Result = "reported" };
        var snapshot = new Snapshot { DeviceId = "first", Connected = true, HeadphoneAccommodations = state };
        var edit = new AccommodationsEditState();
        edit.Update(snapshot, true);
        check(edit.Draft?.Preset is null && edit.Enabled, "An unknown curve must not become an Audiogram or stock tuning.");
        var sentEdit = new AccommodationsEditState();
        var sent = state with { EditVersion = 3, Result = "sent",
            Requested = new() { Phone = false, Media = true, Preset = new() { Tuning = "brightness", Optimization = "strong" } } };
        var sentSnapshot = new Snapshot { DeviceId = "sent-pair", Connected = true, HeadphoneAccommodations = sent };
        sentEdit.Update(sentSnapshot, true);
        check(sentEdit.Baseline?.Phone == true && sentEdit.Draft is { Phone: false, Media: true, Preset.Tuning: "brightness" } && !sentEdit.Dirty,
            "Display the last successful choice while keeping the received baseline intact.");
        sentEdit.Enable(false);
        check(sentEdit.Command(90) is { Phone: null, Media: false, Preset: null, Session: 17, Revision: 4, EditVersion: 3 },
            "The next change must compare against the sent selection and carry both freshness tokens.");
        sentSnapshot.HeadphoneAccommodations = sent with { EditVersion = 4,
            Requested = sent.Requested! with { Phone = true, Media = false } };
        sentEdit.Update(sentSnapshot, true);
        check(!sentEdit.Dirty && sentEdit.Command(91) is null && sentEdit.Draft is { Phone: true, Media: false },
            "A newer local send supersedes another editor's obsolete draft.");
        sentEdit.Enable(false);
        sentSnapshot.HeadphoneAccommodations = state with { Revision = 5, EditVersion = 5, Phone = false, Media = false, Requested = null };
        sentEdit.Update(sentSnapshot, true);
        check(!sentEdit.Dirty && !sentEdit.Enabled && sentEdit.Draft?.Preset is null,
            "A fresh device report overrides local requests, including a disagreeing report.");
        sentEdit.Update(sentSnapshot, false);
        check(sentEdit.Draft is null && sentEdit.Command(92) is null, "Disconnect discards the local send view.");
        var personalEdit = new AccommodationsEditState();
        var personalSnapshot = new Snapshot { DeviceId = "personal-pair", Connected = true, HeadphoneAccommodations = state };
        personalEdit.Update(personalSnapshot, true);
        personalEdit.Edit(personalEdit.Draft! with { Audiogram = true, Preset = null });
        check(personalEdit.Command(93) is null, "Older backends and missing stored profiles cannot authorize Audiogram selection.");
        personalSnapshot.HeadphoneAccommodations = state with { AudiogramAvailable = true };
        personalEdit.Update(personalSnapshot, true);
        personalEdit.Edit(personalEdit.Draft! with { Audiogram = true, Preset = null });
        check(personalEdit.Command(93) is { Audiogram: true, Preset: null, Phone: null, Media: null },
            "Audiogram selection must explicitly request its stored source, without inventing a stock optimization.");
        personalSnapshot.HeadphoneAccommodations = state;
        personalEdit.Update(personalSnapshot, true);
        check(!personalEdit.Dirty && personalEdit.Command(93) is null,
            "Losing the validated source must retire an unsent Audiogram draft.");
        personalSnapshot.HeadphoneAccommodations = state with { EditVersion = 1, Result = "sent",
            Requested = new() { Phone = true, Media = false, Audiogram = true } };
        personalEdit.Update(personalSnapshot, true);
        personalEdit.Enable(false);
        check(personalEdit.Command(94) is { Audiogram: false, Preset: null, Phone: false, EditVersion: 1 } && personalEdit.Draft?.Audiogram == true,
            "Flags-only edits preserve the selected personal tuning without re-importing it.");
        personalEdit.Edit(personalEdit.Draft! with { Audiogram = false, Preset = new() { Tuning = "balanced_tone", Optimization = "slight" } });
        check(personalEdit.Command(95) is { Audiogram: false, Preset.Tuning: "balanced_tone" },
            "A stock selection explicitly replaces personal tuning.");
        personalSnapshot.HeadphoneAccommodations = state with { Revision = 5, EditVersion = 2 };
        personalEdit.Update(personalSnapshot, true);
        check(personalEdit.Draft is { Audiogram: false, Preset: null } && !personalEdit.Dirty,
            "An unmatched fresh record must not inherit the prior Audiogram label.");
        edit.Enable(false);
        check(!edit.Enabled && edit.Command(1) is { Phone: false, Media: null, Preset: null, Session: 17, Revision: 4 },
            "Master off must change only the active destination and preserve unknown curves.");
        edit.Enable(true);
        check(edit.Draft is { Phone: true, Media: false } && !edit.Dirty, "Master on must restore the independent destination choices.");
        edit.Edit(state with { Preset = new() { Tuning = "vocal_range", Optimization = "moderate" } });
        snapshot.HeadphoneAccommodations = state with { };
        edit.Update(snapshot, true);
        check(edit.Baseline?.Revision == 4 && edit.Command(2) is { Phone: null, Media: null, Preset.Tuning: "vocal_range" },
            "An identical poll must retain an unsent edit and its exact baseline.");
        snapshot.HeadphoneAccommodations = state with { Result = "not_sent", Error = "Not sent." };
        edit.Update(snapshot, true);
        check(edit.Dirty && edit.Draft?.Preset?.Tuning == "vocal_range" && edit.Baseline?.Revision == 4,
            "Error-only metadata must not discard an unsent edit.");
        snapshot.HeadphoneAccommodations = state with { Revision = 5, Phone = false, Media = false };
        edit.Update(snapshot, true);
        check(edit.Baseline?.Revision == 5 && !edit.Enabled && !edit.Dirty && edit.Command(2) is null && edit.Draft?.Preset is null,
            "A fresh changed all-off report must replace an obsolete enabled draft.");
        edit.Enable(true);
        snapshot.HeadphoneAccommodations = snapshot.HeadphoneAccommodations with { Media = null, Editable = false };
        edit.Update(snapshot, true);
        check(edit.Command(3) is null && edit.Dirty, "Partial freshness must disable writing without replacing an unsent draft.");
        snapshot.DeviceId = "second";
        snapshot.HeadphoneAccommodations = state with { Session = 18, Phone = false, Media = false };
        edit.Update(snapshot, true);
        check(!edit.Dirty && !edit.Enabled && edit.Draft?.Session == 18, "Another pair/session cannot inherit a draft.");
        edit.Enable(true);
        check(edit.Enabled && edit.Draft is { Phone: true, Media: true }, "Enabling an initially off pair must make an explicit destination choice.");
        edit.Update(snapshot, false);
        check(edit.Draft is null && !edit.Dirty, "Disconnect clears accommodation drafts and identity.");
        var received = new AccommodationsEditState();
        var readOnly = state with { Editable = false, Preset = new() { Tuning = "brightness", Optimization = "strong" } };
        received.Update(new Snapshot { DeviceId = "reported-read-only", HeadphoneAccommodations = readOnly }, true);
        check(received.Enabled && received.PhoneChoice == true && received.MediaChoice == false &&
            received.Draft?.Preset == readOnly.Preset && received.Command(9) is null,
            "A valid read-only report must display its actual destinations and stock tuning without enabling writes.");
        var receivedOff = readOnly with { Phone = false, Media = false };
        var receivedView = received.View(receivedOff);
        check(!receivedView.Enabled && receivedView.Phone == false && receivedView.Media == false &&
            receivedView.Settings == receivedOff,
            "A complete read-only all-off report must not display remembered destinations as enabled.");
        var dirtyDisplay = new AccommodationsEditState();
        dirtyDisplay.Update(new Snapshot { DeviceId = "same-pair", HeadphoneAccommodations = state }, true);
        dirtyDisplay.Edit(state with { Phone = false, Media = true, Preset = new() { Tuning = "balanced_tone", Optimization = "slight" } });
        var changedReadOnly = state with { Revision = 5, Editable = false, Preset = readOnly.Preset };
        dirtyDisplay.Update(new Snapshot { DeviceId = "same-pair", HeadphoneAccommodations = changedReadOnly }, true);
        var actualView = dirtyDisplay.View(changedReadOnly);
        check(actualView.Settings == changedReadOnly && actualView.Phone == true && actualView.Media == false &&
            actualView.Settings.Preset == readOnly.Preset && dirtyDisplay.Command(10) is null,
            "A new read-only record must display actual flags and stock tuning instead of an unsent draft.");
        var opaque = new AccommodationsEditState();
        opaque.Update(new Snapshot { DeviceId = "custom-pair", HeadphoneAccommodations = state }, true);
        opaque.Edit(state with { Preset = new() { Tuning = "brightness", Optimization = "strong" } });
        opaque.Update(new Snapshot { DeviceId = "custom-pair", HeadphoneAccommodations = state with { Revision = 5 } }, true);
        check(!opaque.Dirty && opaque.Baseline?.Revision == 5 && opaque.Draft?.Preset is null && opaque.Command(11) is null,
            "A newer received revision must discard a draft even when changed custom curves have the same null preset projection.");
        foreach (var (phone, media) in new[] { (true, false), (false, true) })
        {
            var saved = new Dictionary<string, (bool Phone, bool Media)>();
            AccommodationsEditState Create() => new(id => saved.TryGetValue(id, out var choice) ? choice : null,
                (id, p, m) => saved[id] = (p, m));
            var policy = Create();
            var current = new Snapshot { DeviceId = "pair-a", HeadphoneAccommodations = state with { Phone = phone, Media = media } };
            policy.Update(current, true);
            policy.Enable(false);
            check(policy.PhoneChoice == phone && policy.MediaChoice == media && !policy.Enabled,
                "Master off must keep the visible independent choices while sending disabled wire flags.");
            policy.Begin(current);
            current.HeadphoneAccommodations = state with { Session = 18, Phone = false, Media = false, Result = "confirmed" };
            policy.Update(current, true);
            policy.Enable(true);
            check(policy.Draft?.Phone == phone && policy.Draft?.Media == media,
                "A controlled refresh after master off must retain Phone-only or Media-only preferences.");
            var restarted = Create();
            restarted.Update(current, true);
            restarted.Enable(true);
            check(restarted.Draft?.Phone == phone && restarted.Draft?.Media == media,
                "Persisted destination choices must survive a new editor instance.");
            current.DeviceId = "pair-b";
            current.HeadphoneAccommodations = state with { Session = 19, Phone = !phone, Media = !media };
            restarted.Update(current, true);
            restarted.Enable(false);
            current.DeviceId = "pair-a";
            current.HeadphoneAccommodations = state with { Session = 20, Phone = false, Media = false };
            restarted.Update(current, true);
            restarted.Edit(restarted.Draft! with { Preset = new() { Tuning = "brightness", Optimization = "strong" } });
            current.HeadphoneAccommodations = current.HeadphoneAccommodations with { Error = "Not sent.", Result = "not_sent" };
            restarted.Update(current, true);
            check(restarted.Dirty && restarted.PhoneChoice == phone && restarted.MediaChoice == media,
                "Errors must not replace unsent drafts or copy another pair's remembered choices.");
            restarted.Begin(current);
            current.HeadphoneAccommodations = current.HeadphoneAccommodations with { Pending = true, Editable = false };
            restarted.Update(current, true);
            current.HeadphoneAccommodations = current.HeadphoneAccommodations with { Pending = false, Result = "unconfirmed", Error = "Refresh required." };
            restarted.Update(current, true);
            check(!restarted.Requesting && restarted.Command(8) is null && restarted.PhoneChoice == phone && restarted.MediaChoice == media,
                "An unconfirmed send must keep local choices and require fresh editable state.");
        }
        var none = new AccommodationsEditState(_ => (false, false));
        none.Update(new Snapshot { DeviceId = "saved-none", HeadphoneAccommodations = state with { Phone = false, Media = false } }, true);
        none.Enable(true);
        check(!none.Enabled && none.PhoneChoice == false && none.MediaChoice == false && !none.Dirty,
            "An explicitly saved empty destination choice must not be replaced by first-use defaults.");
        none.Destinations(true, false);
        none.Enable(true);
        check(none.Enabled && none.Draft is { Phone: true, Media: false }, "A destination choice allows an explicit master enable.");
        none.Begin(new Snapshot { HeadphoneAccommodations = state });
        none.UseCachedReport(new Snapshot { HeadphoneAccommodations = state });
        check(!none.Requesting && !none.Dirty, "An unchanged cached projection must not wait for write-style acknowledgement.");
        var persisted = new Dictionary<string, (bool Phone, bool Media)> { ["001122334455"] = (true, false), ["aabbccddeeff"] = (false, true) };
        AccommodationsEditState UsbEditor() => new(key => persisted.TryGetValue(key, out var choice) ? choice : null,
            (key, p, m) => persisted[key] = (p, m));
        var usb = UsbEditor();
        var usbState = new Snapshot { DeviceId = "dongle:7:1", HeadphoneAccommodations = state with { Phone = false, Media = false } };
        usb.Update(usbState, true, "001122334455");
        usbState.DeviceId = "dongle:8:2";
        usbState.HeadphoneAccommodations = usbState.HeadphoneAccommodations with { Session = 19 };
        usb.Update(usbState, true, "001122334455");
        usb.Enable(true);
        check(usb.Draft is { Phone: true, Media: false }, "USB routing generations must not replace real-pair preferences.");
        var usbRestart = UsbEditor();
        usbRestart.Update(usbState, true, "001122334455");
        usbRestart.Enable(true);
        check(usbRestart.Draft is { Phone: true, Media: false }, "Native and USB editor restarts must share the actual pair's persisted choices.");
        usb.Update(usbState, true, "aabbccddeeff");
        check(!usb.Dirty && usb.PhoneChoice == false && usb.MediaChoice == true, "A replaced USB peer must discard the prior pair's draft and choices.");
        usb.Update(usbState, true);
        check(!usb.Dirty && usb.PhoneChoice == true && usb.MediaChoice == true,
            "Cleared USB identity must not reuse the previous pair's preference key.");
        var live = new AccommodationsEditState();
        var liveState = new Snapshot { DeviceId = "live", Connected = true, HeadphoneAccommodations = state };
        live.Update(liveState, true);
        live.Enable(false);
        live.Begin(liveState);
        live.Enable(true);
        liveState.HeadphoneAccommodations = state with { Pending = true, Editable = false };
        live.Update(liveState, true);
        check(live.CanEdit && live.Sending && live.Dirty && live.View(liveState.HeadphoneAccommodations).Enabled,
            "An on/off reversal while a write runs must keep the newest visible choice.");
        liveState.HeadphoneAccommodations = state with { EditVersion = 1, Result = "sent",
            Requested = new() { Phone = false, Media = false } };
        live.Update(liveState, true);
        check(!live.Sending && live.Dirty && live.Command(100) is { Phone: true, EditVersion: 1 },
            "A queued toggle uses the completed send's edit token without reconnecting.");
        live.Begin(liveState);
        live.Enable(false);
        liveState.HeadphoneAccommodations = state with { Session = 18 };
        live.Update(liveState, true);
        check(!live.Sending && !live.Dirty && live.Command(101) is null,
            "A new session must discard an in-flight toggle and its queued reversal.");
        live.Enable(false);
        live.Begin(liveState);
        live.Enable(true);
        liveState.HeadphoneAccommodations = liveState.HeadphoneAccommodations with {
            EditVersion = 2, Requested = new() { Phone = false, Media = false },
            Result = "unconfirmed", Error = "Verification failed." };
        live.Update(liveState, true);
        check(!live.Sending && !live.Dirty && live.Command(102) is null,
            "Matching requested data with a failed transaction must discard the queued toggle.");
        var fastFailure = new AccommodationsEditState();
        var failedState = new Snapshot { DeviceId = "same-error", Connected = true,
            HeadphoneAccommodations = state with { Error = "The same failure", Result = "not_sent" } };
        fastFailure.Update(failedState, true);
        fastFailure.Enable(false);
        fastFailure.Begin(failedState, 200, 7);
        fastFailure.Enable(true);
        fastFailure.Update(failedState, true);
        check(fastFailure.Sending && fastFailure.Dirty,
            "An unchanged error and an omitted pending snapshot cannot identify this request's outcome.");
        var failedResult = new DaemonEvent.CommandResult("set_headphone_accommodations", 200, false, "The same failure");
        check(!fastFailure.FailRequest(failedResult with { RequestId = 201 }, 7) &&
            !fastFailure.FailRequest(failedResult, 6) && fastFailure.Sending,
            "Another request or an old client epoch must not release the current edit.");
        check(fastFailure.FailRequest(failedResult, 7) && !fastFailure.Sending && !fastFailure.Dirty,
            "A correlated repeated failure settles immediately and discards its queued successor.");
        fastFailure.Enable(false);
        fastFailure.Begin(failedState, 202, 7);
        fastFailure.Enable(true);
        check(!fastFailure.FailRequest(failedResult with { RequestId = 202, Ok = true }, 7) && fastFailure.Sending,
            "A successful command result still waits for the settings report; transmission is not confirmation.");
        fastFailure.Cancel();
        check(!fastFailure.FailRequest(failedResult with { RequestId = 202 }, 7),
            "Unloading or cancelling the editor retires its failure result.");
        fastFailure.Update(failedState, true);
        fastFailure.Enable(false);
        fastFailure.Begin(failedState, 203, 7);
        failedState.DeviceId = "other-pair";
        fastFailure.Update(failedState, true);
        check(!fastFailure.FailRequest(failedResult with { RequestId = 203 }, 7),
            "An old pair's completion cannot release work after the owner changes.");
        fastFailure.Enable(false);
        fastFailure.Begin(failedState, 204, 8);
        check(!fastFailure.FailRequest(failedResult with { RequestId = 203 }, 8) && fastFailure.Sending,
            "An older failure cannot cancel a newer request in the same editor.");
        failedState.HeadphoneAccommodations = failedState.HeadphoneAccommodations! with { Session = 18 };
        fastFailure.Update(failedState, true);
        check(!fastFailure.FailRequest(failedResult with { RequestId = 204 }, 8),
            "A replaced accessory session retires its correlated result.");
        Console.WriteLine("Headphone Accommodations edit checks passed.");
    }
}
