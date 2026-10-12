using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

internal static class ModelCompatibilityChecks
{
    public static void Run(Action<bool, string> check)
    {
        foreach (var (firmware, major) in new[]
        {
            ("6619472", 0), ("90.3431.6807", 0), ("6F21", 6), ("9A5347a", 9),
            ("6.8.8", 6), (" 10A1 ", 10), ("1.0.255", 1), ("99Z1z", 99),
            ("9", 0), ("9A", 0), ("9a123", 0), ("9A1aa", 0), ("9A1A", 0),
            ("0A1", 0), ("100A1", 0), ("6.256.1", 0), ("6.1", 0), ("6.1.1.1", 0),
            ("6.+1.1", 0), ("6.1 .1", 0), ("９A1", 0), ("9A١", 0), ("unknown", 0), ("", 0)
        })
            check(DeviceModels.FirmwareMajor(firmware) == major, $"Firmware '{firmware}' must resolve to major {major}.");
        check(DeviceModels.FirmwareMajor(null) == 0, "Missing firmware must not enable advanced features.");
        foreach (var firmware in new[] { "6619472", "90.3431.6807" })
        {
            var malformed = new Snapshot { Model = "A3063", Firmware = firmware };
            check(!DeviceModels.Adaptive(malformed) && !DeviceModels.HighQualityMic(malformed) && !DeviceModels.CustomEq(malformed),
                "Packed and internal firmware numbers cannot enable modern protocol paths.");
        }
        foreach (var (model, listening, adaptive, headphone, mic, hr) in new[]
        {
            ("A1523", false, false, false, false, false),
            ("A2032", false, false, false, false, false),
            ("A2564", false, false, false, false, false),
            ("A3053", false, false, false, true, false),
            ("A3056", true, true, false, true, false),
            ("A2083", true, false, false, false, false),
            ("A2698", true, true, false, true, false),
            ("A3049", true, true, false, true, false),
            ("A3065", true, true, false, true, true),
            ("A2096", true, false, true, false, false),
            ("A3184", true, false, true, false, false),
            ("A3454", true, true, true, true, false),
            ("A3531", true, true, false, true, false),
            ("A3440", true, true, false, true, false),
            ("A3157", true, false, false, false, false),
            ("unknown", false, false, false, false, false)
        })
        {
            var s = new Snapshot { Model = model, Firmware = "9A348", Connected = true };
            check(DeviceModels.Listening(s) == listening && DeviceModels.Adaptive(s) == adaptive &&
                DeviceModels.Headphone(s) == headphone && DeviceModels.HighQualityMic(s) == mic &&
                DeviceModels.Find(model).HeartRate == hr, $"Model capabilities for {model}.");
            check(!DeviceSettingStatus.CanChange(s, 0x1F), "Supported settings still need actual reports.");
            s.DeviceControls[0x1F] = 0x5032;
            check(DeviceSettingStatus.CanChange(s, 0x1F) == (model != "A3157"),
                "A tone report is usable only on models with a verified tone control, or an unknown model's generic path.");
            if (!adaptive)
            {
                s.DeviceControls[0x2E] = 50;
                check(!DeviceSettingStatus.CanChange(s, 0x2E), "A stray modern report cannot enable unsupported controls.");
            }
            s.Firmware = "7B19";
            check(!DeviceModels.HighQualityMic(s) && !DeviceModels.CustomEq(s), "Old firmware never enables new audio protocols.");
            s.Firmware = "8B34";
            check(!DeviceModels.CustomEq(s), "Firmware 8 must not enable custom EQ.");
        }

        var pro1 = new Snapshot { Model = "A2084", Connected = true };
        check(DeviceSettingStatus.AllowOffEnabled(pro1), "Pro 1 Off does not depend on the newer Allow Off setting.");
        pro1.MicMode = "manual";
        pro1.MicManualActive = true;
        check(MicPolicyState.CanStop(pro1) && !MicPolicyState.CanStart(pro1), "An unsupported model must still allow an existing mic request to stop.");
        var pro2 = new Snapshot { Model = "A2698", Firmware = "6B34", Connected = true };
        check(DeviceModels.OffAvailable(pro2) && !DeviceModels.AllowOffOption(pro2), "Pro 2 before hearing protection retains ordinary Off.");
        pro2.Firmware = "7B19";
        check(!DeviceModels.OffAvailable(pro2) && DeviceModels.AllowOffOption(pro2), "Firmware 7 must respect the hearing-protection Off setting.");
        pro2.DeviceControls[Feature.AllowOff] = 2;
        pro2.DeviceControls[0x0D] = 1;
        check(!DeviceModels.OffAvailable(pro2), "A reported disabled Off option wins over an old mode report.");
        pro2.DeviceControls.Remove(Feature.AllowOff);
        check(DeviceModels.OffAvailable(pro2), "An actual current Off mode remains available when the option was not reported.");
        pro2.Firmware = "5E135";
        check(!DeviceModels.Adaptive(pro2), "Firmware before Adaptive Audio must not show its mode.");
        var unknown = new Snapshot { Connected = true, DeviceControls = new() { [0x0D] = 1 } };
        check(DeviceModels.Listening(unknown) && DeviceSettingStatus.AllowOffEnabled(unknown), "Unknown models may expose a confirmed legacy mode.");
        unknown.DeviceControls[0x0D] = 4;
        check(!DeviceModels.Listening(unknown), "Unknown Adaptive reports must not opt into advanced protocols.");

        check(DeviceModels.Find(" a3065 ").Family == "airpods_pro_3" &&
            DeviceModels.Find("A2698").Name.Contains("Pro 2"), "All model aliases resolve through the shared catalog.");
        var max = new Snapshot { Model = "A3184", Battery = new Battery { Headphone = 77 } };
        check(DeviceModels.Headphone(max) && BatteryPresentation.Reading(max, BatteryPart.Headphone) == 77,
            "AirPods Max uses its headset reading, not empty earbud/case readings.");
        max.Battery.Headphone = null;
        check(DeviceModels.Headphone(max) && DeviceArt.MainImage(max.Model)?.EndsWith("airpods_max.png") == true,
            "A missing Max reading retains the single-headset layout and real Max artwork.");

        foreach (var (model, artworkModel) in new[] { ("A3531", "A3053"), ("A3440", "A3053"), ("A3157", "A2698"), ("unknown", "A2698"), ("", "A2698") })
            check(DeviceArt.MainImage(model) == DeviceArt.MainImage(artworkModel) &&
                DeviceArt.IslandImage(model) == DeviceArt.IslandImage(artworkModel) &&
                DeviceArt.LeftImage(model) == DeviceArt.LeftImage(artworkModel) &&
                DeviceArt.RightImage(model) == DeviceArt.RightImage(artworkModel) &&
                DeviceArt.CaseImage(model) == DeviceArt.CaseImage(artworkModel),
                $"{model} reuses the closest bundled artwork consistently across surfaces.");
        var unknownHeadset = new Snapshot { Battery = new Battery { Headphone = 70 } };
        check(DeviceArt.MainImage(unknownHeadset.Model, DeviceModels.Headphone(unknownHeadset)) == DeviceArt.MainImage("A3184") &&
            DeviceArt.IslandImage(unknownHeadset.Model, DeviceModels.Headphone(unknownHeadset)) == DeviceArt.IslandImage("A3184") &&
            !DeviceModels.Find(unknownHeadset.Model).Known && !DeviceModels.HighQualityMic(unknownHeadset),
            "Unknown headphones reuse Max artwork without acquiring Max identity or features.");
        check(DeviceModels.DisplayName(new Snapshot { Model = "A3157", DevName = "My Powerbeats" }, "Headphones") == "My Powerbeats" &&
            DeviceModels.DisplayName(new Snapshot { Model = "A3157" }, "Headphones") == "Powerbeats Pro 2" &&
            DeviceModels.DisplayName(new Snapshot { Model = "unknown" }, "Headphones") == "Headphones",
            "Reported names take priority; unidentified devices must not acquire AirPods branding.");
        foreach (string firmware in new[] { "", "6619472", "90.3431.6807" })
            check(!DeviceModels.HeadTracking(new Snapshot { Model = "A2083", Firmware = firmware }),
                "Head tracking must wait for recognized public firmware metadata.");
        check(DeviceModels.HeadTracking(new Snapshot { Model = "A2083", Firmware = "6F21" }) &&
            !DeviceModels.HeadTracking(new Snapshot { Model = "A3157", Firmware = "9A348" }),
            "Valid firmware enables only models with the supported head-tracking protocol.");

        var max2 = new Snapshot { Model = "A3454", Firmware = "9A348", Connected = true };
        check(!DeviceModels.CanChangeLoudSoundReduction(max2), "Loud Sound Reduction needs an actual readback before its first write.");
        max2.LoudSoundReduction = false;
        check(DeviceModels.CanChangeLoudSoundReduction(max2) && !DeviceModels.Find(max2.Model).HearingAid,
            "Max 2 can change confirmed Loud Sound Reduction without enabling a hearing aid.");
        max2.LoudSoundReductionPending = true;
        check(!DeviceModels.CanChangeLoudSoundReduction(max2), "Pending writes cannot overlap another Loud Sound Reduction request.");
        max2.LoudSoundReductionPending = false;
        max2.LinkStale = true;
        check(!DeviceModels.CanChangeLoudSoundReduction(max2), "A stale connection cannot change Loud Sound Reduction.");
        max2.LinkStale = false;
        max2.Connected = false;
        check(!DeviceModels.CanChangeLoudSoundReduction(max2), "A disconnected device cannot change Loud Sound Reduction.");
        foreach (string model in new[] { "A2083", "A3157", "A3531", "unknown" })
            check(!DeviceModels.CanChangeLoudSoundReduction(new Snapshot { Model = model, Connected = true, LoudSoundReduction = true }),
                "A stale or stray report cannot grant an unsupported hearing-protection protocol.");

        max.Connected = true;
        max.DeviceControls[0x1C] = 2;
        max.DeviceControls[0x24] = 0x0300;
        max.DeviceControls[0x01] = 0;
        check(DeviceSettingStatus.CanChange(max, 0x1C) && DeviceSettingStatus.CanChange(max, 0x24),
            "Reported Max Crown and call controls are available.");
        check(!DeviceSettingStatus.CanChange(max, 0x01), "Max must not display an unverified left/right earbud microphone policy.");
        pro1.DeviceControls[0x1C] = 2;
        check(!DeviceSettingStatus.CanChange(pro1, 0x1C), "An earbud model cannot expose Digital Crown controls.");
        foreach (var model in new[] { "A1523", "A2032" })
        {
            var early = new Snapshot { Model = model, Connected = true, DeviceControls = new() { [0x3B] = 1, [0x04] = 1, [0x0C] = 1 } };
            check(!DeviceSettingStatus.CanChange(early, 0x3B) && !DeviceSettingStatus.CanChange(early, 0x04) &&
                !DeviceSettingStatus.CanChange(early, 0x0C), "Early AirPods must not expose unsupported charging or guessed double-tap writes.");
        }
        max2.Connected = true;
        max2.Firmware = "7B19";
        max2.DeviceControls[0x35] = 1;
        check(!DeviceSettingStatus.CanChange(max2, 0x35), "Sleep detection requires supported firmware even with a stray report.");
        max2.Firmware = "8B34";
        check(DeviceSettingStatus.CanChange(max2, 0x35), "Max 2 sleep detection is available after a supported firmware report.");
        foreach (var (model, swipe) in new[] { ("A3531", false), ("A3439", true) })
        {
            var fifth = new Snapshot { Model = model, Firmware = "9A348", Connected = true, DeviceControls = new() { [0x25] = 1 } };
            check(DeviceSettingStatus.CanChange(fifth, 0x25) == swipe, "Only the wireless-case AirPods 5 exposes volume swipe.");
            fifth.DeviceControls[Feature.AllowOff] = 2;
            check(!DeviceModels.OffAvailable(fifth), "An explicit disabled Off report overrides the legacy default.");
            fifth.DeviceControls[Feature.AllowOff] = 1;
            check(DeviceModels.OffAvailable(fifth), "An explicit enabled Off report permits Off.");
            fifth.DeviceControls[Feature.AllowOff] = 0;
            check(DeviceModels.OffAvailable(fifth), "An invalid Off report must not remove the legacy default.");
        }

        var art = new DeviceArtworkState();
        check(art.Update(new Snapshot { DeviceId = "first", Model = "A3063" }, "", "") == "A3063", "Initial artwork follows metadata.");
        check(art.Update(new Snapshot { DeviceId = "first" }, "A2083", "other") == "A3063", "Nearby reports retain the same device artwork.");
        check(art.Update(new Snapshot { DeviceId = "second" }, "A3063", "first") == "", "Another device cannot borrow the previous model.");
        check(art.Update(new Snapshot { DeviceId = "second" }, "A2083", "second") == "A2083", "Saved artwork must match the selected device identity.");

        var eq = new EqualizerEditState();
        eq.Update(new Snapshot { DeviceId = "first" }, true);
        eq.Edit();
        eq.Update(new Snapshot { DeviceId = "second" }, true);
        check(!eq.Dirty && eq.Requested is null, "An EQ draft must not transfer to another AirPods pair.");
        var policy = new MicPolicyState();
        policy.Update(new Snapshot { DeviceId = "first" });
        policy.QueuePolicy("keep_active", 30);
        policy.Update(new Snapshot { DeviceId = "second" });
        check(policy.PendingPolicy is null, "A queued microphone policy must not transfer to another pair.");
        Console.WriteLine("AirPods model, firmware, identity and Max battery checks passed.");
    }
}
