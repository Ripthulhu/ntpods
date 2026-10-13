using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Popup;
using NTPods.WinUI.Services;

internal static class UiInteractionChecks
{
    public static void Run(Action<bool, string> check)
    {
        foreach (double scale in new[] { 1d, 1.25, 1.5, 2, 3 })
        {
            var minimum = WindowSizing.Minimum(scale, 16, 39, 8000, 8000);
            check(minimum.Width - 16 >= 420 * scale && minimum.Height - 39 >= 540 * scale,
                "The minimum client area must preserve logical size at each monitor scale.");
            check(minimum.Width == (int)Math.Ceiling(420 * scale) + 16,
                "Window borders must be added once outside the scaled client area.");
        }
        check(WindowSizing.Minimum(2, 16, 39, 800, 1000) == (800, 1000),
            "A small monitor must cap the minimum to its available work area.");
        check(WindowSizing.Minimum(1, 16, 39, 1920, 1040) == (436, 579),
            "Moving back to a 100% monitor must restore the smaller minimum.");
        check(WindowSizing.Minimum(double.NaN, 0, 0, 1920, 1040) == (420, 540),
            "Unavailable DPI must use the standard scale.");

        var reduction = new Snapshot { Model = "A3063", Connected = true };
        check(DeviceModels.ShouldReadLoudSoundReduction(reduction, false, false), "An unknown setting is read when first opened.");
        check(!DeviceModels.ShouldReadLoudSoundReduction(reduction, true, false), "Ordinary snapshots must not repeatedly retry a read.");
        reduction.LoudSoundReduction = true;
        check(!DeviceModels.ShouldReadLoudSoundReduction(reduction, false, false) &&
            DeviceModels.ShouldReadLoudSoundReduction(reduction, true, true),
            "Explicit refresh must reread an already cached value while automatic rendering leaves it alone.");
        reduction.LoudSoundReductionPending = true;
        check(!DeviceModels.ShouldReadLoudSoundReduction(reduction, true, true), "Refresh must not overlap a pending ATT transaction.");
        reduction.LoudSoundReductionPending = false;
        reduction.LinkStale = true;
        check(!DeviceModels.ShouldReadLoudSoundReduction(reduction, true, true), "Refresh must not use a stale connection.");
        reduction.LinkStale = false; reduction.Connected = false;
        check(!DeviceModels.ShouldReadLoudSoundReduction(reduction, true, true), "Refresh must not connect a sleeping pair.");
        reduction.Connected = true; reduction.Model = "A2083";
        check(!DeviceModels.ShouldReadLoudSoundReduction(reduction, true, true), "Explicit refresh still respects model support.");

        var identity = new DeviceViewIdentity();
        check(!identity.Update(new Snapshot()) && !identity.Update(new Snapshot { DeviceId = "pro3", Model = "A3063" }),
            "Initial state is covered by the app entrance, not a device-switch transition.");
        check(!identity.Update(new Snapshot { DeviceId = "pro3", Model = "a3063", Connected = true }) &&
              !identity.Update(new Snapshot { DeviceId = "pro3", BatteryLive = true }) && !identity.Update(new Snapshot()),
            "Status and partial battery reports must not restart the device transition.");
        check(!identity.Update(new Snapshot { DeviceId = "pro3", Model = "A3064" }) &&
              !identity.Update(new Snapshot { DeviceId = "pro3", Model = "A3063" }),
            "A dominant-bud change within the same model family must not animate the page.");
        check(identity.Update(new Snapshot { DeviceId = "pro1", Model = "A2083" }),
            "A different paired device must reveal its header, artwork and controls together.");
        check(!identity.Update(new Snapshot { DeviceId = "pro1", Model = "A2083", LinkStale = true }),
            "A stale-link report must not animate the page.");
        check(identity.Update(new Snapshot { DeviceId = "pro3", Model = "A3063" }) &&
              identity.Update(new Snapshot { DeviceId = "pro1", Model = "A2083" }),
            "Rapid switches must follow the newest identity without delaying its state.");
        check(identity.Update(new Snapshot { DeviceId = "pro1", Model = "A2698" }) &&
              !identity.Update(new Snapshot { DeviceId = "pro1", Model = "A2698" }),
            "Corrected model metadata needs one transition; repeated reports do not.");
        check(identity.Update(new Snapshot { DeviceId = "max" }) &&
              identity.Update(new Snapshot { DeviceId = "max", Model = "A2096" }),
            "Model metadata arriving after identity must also reveal the changed artwork and capabilities.");

        string model = DeviceArt.ResolveModel("A3063", "", "");
        string nearby = DeviceArt.ResolveModel("", model, "A2083");
        check(nearby == model && DeviceArt.CaseImage(nearby) == DeviceArt.CaseImage(model),
            "Nearby reports without model metadata must retain the connected device artwork.");
        check(DeviceArt.ResolveModel("", "", "A3063") == model,
            "Opening the app during nearby charging must use the saved model.");
        check(DeviceArt.ResolveModel("A2083", model, model) == "A2083",
            "New model metadata must replace previously cached artwork.");
        string unknown = DeviceArt.ResolveModel("unsupported", model, model);
        check(DeviceArt.CaseImage(unknown) == DeviceArt.CaseImage("A2698") && DeviceArt.ResolveModel("", unknown, model) == unknown,
            "An explicit unknown model retains its identity while using the shared in-ear artwork fallback.");
        check(DeviceArt.CaseImage(DeviceArt.ResolveModel("", "", "")) == DeviceArt.CaseImage("A2698"),
            "A first-time unknown device uses the same bundled artwork fallback.");

        var volume = new VolumeEditState();
        volume.Update(20, true);
        volume.BeginInteraction();
        volume.Request(40);
        volume.Request(70);
        volume.Update(40, true);
        check(volume.Displayed == 70, "A delayed volume reply must not pull an active drag backwards.");
        volume.Update(70, true);
        check(volume.Requested == 70, "A matching intermediate reply must not release an active gesture.");
        volume.Update(40, true);
        volume.EndInteraction();
        check(volume.Displayed == 70 && volume.Requested == 70, "Release must retain the final value until confirmed.");
        volume.Update(70, true);
        check(volume.Requested is null && volume.Displayed == 70, "The final report releases the local edit.");
        volume.Update(65, true);
        check(volume.Displayed == 65, "Volume changes elsewhere must appear after the edit settles.");
        volume.Request(80);
        volume.Update(65, true);
        check(volume.Displayed == 80, "Keyboard and automation edits also survive delayed reports.");
        volume.Cancel();
        check(volume.Displayed == 65 && !volume.Interacting, "A timeout or failed send restores the last reported volume.");
        volume.BeginInteraction();
        volume.Request(0);
        volume.Update(0, true);
        volume.EndInteraction();
        check(volume.Displayed == 0 && volume.Requested is null, "Zero volume can be confirmed before release.");
        volume.BeginInteraction();
        volume.Request(100);
        volume.Update(30, false);
        check(volume.Displayed == 30 && volume.Requested is null && !volume.Interacting,
            "Disconnect must discard the gesture instead of replaying it after reconnect.");

        var normal = TrayMenuLayout.Fit(1900, 1030, 260, 440, 0, 0, 1920, 1040);
        check(normal == (1660, 590, 260, 440), "A normal tray menu retains its position above the tray.");
        var tall = TrayMenuLayout.Fit(1900, 1000, 780, 1500, 0, 0, 1920, 1040);
        check(tall == (1140, 0, 780, 1040), "A scaled menu taller than the monitor needs a bounded scroll viewport.");
        var leftMonitor = TrayMenuLayout.Fit(-10, 830, 260, 440, -1920, -200, 1920, 1040);
        check(leftMonitor == (-260, 390, 260, 440), "A monitor with negative coordinates must use its own work area.");
        var narrow = TrayMenuLayout.Fit(-1800, -150, 260, 500, -1920, -200, 200, 300);
        check(narrow == (-1920, -200, 200, 300), "Oversized menus must stay inside both work-area axes.");
        Console.WriteLine("Volume edit and tray viewport checks passed.");
    }
}
