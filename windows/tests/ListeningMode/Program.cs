using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

LensChecks.Run();

static void Check(bool condition, string message)
{
    if (!condition) throw new InvalidOperationException(message);
}

foreach (double width in new[] { 268d, 360d, 640d })
{
    var layout = ListeningModeLayout.Create(width, 4, false);
    double minimum = layout.AdaptiveStart + 22.5, maximum = layout.AncStart - 22.5;
    Check(layout.MarkerOpacity(minimum, layout.LeftFor(4, 0), 1) == 0 &&
        layout.MarkerOpacity(maximum, layout.LeftFor(4, 100), 1) == 0,
        "Covered Adaptive endpoints must not show through or refract at the thumb rim");
    Check(layout.MarkerOpacity(minimum, layout.LeftFor(4, 50), 1) == 1 &&
        layout.MarkerOpacity(maximum, layout.LeftFor(4, 50), 1) == 1,
        "Uncovered Adaptive endpoints remain visible");
    double previous = 0;
    for (double left = layout.LeftFor(4, 0); left <= layout.LeftFor(4, 50); left += .1)
    {
        double opacity = layout.MarkerOpacity(minimum, left, 1);
        Check(opacity >= previous && opacity - previous < .02, "Endpoint reveal must remain continuous and monotonic");
        Check(layout.MarkerOpacity(minimum, left, 1.8) <= opacity, "Press expansion must also cover the endpoint");
        Check(layout.MarkerOpacity(minimum, left, .7) <= opacity, "Release compression must not reveal a marker inside the lens source");
        previous = opacity;
    }
}

foreach (double width in new[] { 1d, 360d, 640d })
{
    foreach (bool allowOff in new[] { false, true })
    {
        var collapsed = ListeningModeLayout.Create(width, 3, allowOff);
        Check(Math.Abs(collapsed.Segment * (allowOff ? 4 : 3) - width) < 0.00001, "Collapsed modes must fill the track");
        Check((collapsed.OffWidth > 0) == allowOff, "Off must have no space when disabled");
        foreach (uint mode in new uint[] { 1, 3, 4, 2 })
        {
            if (mode == 1 && !allowOff) continue;
            Check(collapsed.ModeAt(collapsed.LeftFor(mode, 50)) == mode, "Each collapsed segment must select its protocol mode");
        }
        var expanded = ListeningModeLayout.Create(width, 4, allowOff);
        Check(expanded.OffWidth == 0, "Expanded Adaptive hides Off even when enabled");
        Check(Math.Abs(expanded.AdaptiveWidth - width * 3 / 5) < 0.00001, "Adaptive must occupy three of five segments");
        foreach (uint strength in new uint[] { 0, 1, 25, 50, 75, 99, 100 })
        {
            double left = expanded.LeftFor(4, strength);
            Check(expanded.StrengthAt(left) == strength, "Adaptive strength must round-trip across widths");
            Check(expanded.ModeAt(left) == 4, "The full strength range must stay inside Adaptive");
        }
        Check(expanded.ModeAt(0) == 3 && expanded.ModeAt(width) == 2, "Dragging beyond Adaptive must reach both neighboring modes");
        Check(expanded.StrengthAt(-100) == 0 && expanded.StrengthAt(width * 2) == 100, "Out-of-bounds strength must clamp");
    }
}
foreach (double width in new[] { 0d, -1d, double.NaN, double.PositiveInfinity })
{
    var layout = ListeningModeLayout.Create(width, 4, true);
    Check(layout.Width == 0 && layout.LeftFor(4, 50) == 0 && layout.StrengthAt(10) == 0, "Unmeasured layout must remain finite and inert");
}

var collapsedFromAdaptive = ListeningModeLayout.Create(600, 4, true).ResolveDrag(50, 50, true);
Check(collapsedFromAdaptive.Mode == 3 && collapsedFromAdaptive.Left == 50,
    "Leaving Adaptive must select adjacent Transparency without moving the thumb or skipping to Off");
foreach (double width in new[] { 360d, 600d, 640d })
foreach (bool allowOff in new[] { false, true })
foreach (uint initialMode in new uint[] { 1, 2, 3, 4 })
for (double left = 0; left <= width; left += 1)
{
    var before = ListeningModeLayout.Create(width, initialMode, allowOff);
    var preview = before.ResolveDrag(left, 50, allowOff);
    Check(preview.Mode == before.ModeAt(left), "One movement may cross a boundary only in the current layout");
    Check(Math.Abs(preview.Left - before.ClampLeft(left)) < 0.00001, "Hit testing must preserve the pointer within the displayed layout");
    Check(allowOff || preview.Mode != 1, "Dragging must never preview disabled Off");
}
for (uint strength = 0; strength <= 100; strength++)
{
    var layout = ListeningModeLayout.Create(600, 4, false);
    double left = layout.LeftFor(4, strength);
    var preview = layout.ResolveDrag(left, 50, false);
    Check(preview.Mode == 4 && preview.Strength == strength && preview.Left == left, "Every Adaptive percentage must track the pointer without calibration or snapping");
}

foreach (var model in new[] { "A2083", "A2084", "A2096", "A3184" })
{
    var legacy = new Snapshot { Model = model, Connected = true, DeviceControls = new() { [0x0D] = 2 } };
    foreach (uint mode in new uint[] { 1, 2, 3 })
        Check(ListeningModePolicy.CanSend(legacy, false, 0x0D, mode), "Legacy ANC models expose Off, Transparency and Noise Cancellation.");
    Check(!ListeningModePolicy.CanSend(legacy, false, 0x0D, 4), "Pro 1 and Max 1 must never send Adaptive.");
    var unsupported = new ListeningModeRequest();
    unsupported.QueueGesture(4, 50);
    Check(unsupported.TakeNext(legacy) is null && !unsupported.HasTarget, "An unsupported queued mode must be discarded, not retried forever.");
}
foreach (double width in new[] { 268d, 640d })
foreach (bool off in new[] { false, true })
{
    var layout = ListeningModeLayout.Create(width, 3, off, adaptive: false);
    Check(!layout.HasAdaptive && layout.AdaptiveWidth == 0, "Legacy geometry must leave no Adaptive space.");
    Check(Math.Abs(layout.Segment * 2 + layout.OffWidth - width) < 1e-9, "Legacy modes fill the whole rail.");
    foreach (uint mode in new uint[] { 1, 3, 2 })
    {
        if (mode == 1 && !off) continue;
        Check(layout.ModeAt(layout.LeftFor(mode, 0)) == mode, "Each legacy mode must round-trip through gesture geometry.");
    }
    for (double left = 0; left <= width; left++)
        Check(layout.ResolveDrag(left, 0, off).Mode != 4, "Dragging anywhere on a legacy rail cannot select Adaptive.");
    var motion = new ListeningModeMotion();
    motion.SetTarget(ListeningModeLayout.Create(width, 4, true), 0, 0, false, true);
    motion.SetTarget(layout, layout.LeftFor(2, 0), 10, false, true);
    Check(!motion.Layout.HasAdaptive && motion.Layout.AdaptiveWidth == 0 &&
        Math.Abs(motion.Layout.OffWidth - layout.OffWidth) < 1e-9 && motion.Layout.Segment == layout.Segment && !motion.IsMoving,
        "Switching models replaces unsupported geometry immediately.");
    motion.SetTarget(layout, 0, 20, false, true);
    for (int now = 21; now <= 600; now++)
    {
        motion.Tick(now);
        Check(motion.Layout.AdaptiveWidth == 0 && motion.Layout.ModeAt(motion.Left) != 4,
            "Legacy mode animations cannot introduce an Adaptive region.");
    }
    Check(!motion.IsMoving, "Legacy animations must settle without continuous rendering.");
}

static Snapshot Ready() => new()
{
    Model = "A3063", Firmware = "9A348",
    Connected = true,
    DeviceControls = new() { [0x0D] = 4, [ControlId.AdaptiveNoiseStrength] = 50, [Feature.AllowOff] = 1 }
};

Check(ListeningModePolicy.CanSend(Ready(), false, 0x0D, 1), "Reported Off must be selectable");
Check(ListeningModePolicy.CanSend(Ready(), false, ControlId.AdaptiveNoiseStrength, 100), "Confirmed Adaptive accepts strength");
Check(!ListeningModePolicy.CanSend(Ready(), false, ControlId.AdaptiveNoiseStrength, 101), "Strength must be bounded");
Check(!ListeningModePolicy.CanSend(Ready(), false, 0x0D, 5), "Invalid mode must never be sent");
Check(!ListeningModePolicy.CanSend(Ready(), false, 0xFF, 1), "This control must not send unrelated settings");
Check(!ListeningModePolicy.CanSetStrength(Ready(), true), "Local request must block another write without blocking the gesture");
Check(ListeningModePolicy.CanContinueGesture(Ready()), "A connected device allows local gesture previews");
Check(!ListeningModePolicy.CanShowSelection(1, false), "Disabled Off must not paint over Transparency");
Check(ListeningModePolicy.CanShowSelection(1, true), "Enabled Off must keep its selection");
Action<Snapshot>[] unsafeStates =
[
    state => state.Connected = false,
    state => state.LinkStale = true,
    state => state.AncUnconfirmed = true,
    state => state.DeviceControls.Remove(0x0D),
    state => state.DeviceControls[0x0D] = 3,
    state => state.DeviceControls.Remove(ControlId.AdaptiveNoiseStrength),
    state => state.DeviceControlsPending[0x0D] = 4,
    state => state.DeviceControlsPending[ControlId.AdaptiveNoiseStrength] = 75
];
foreach (var change in unsafeStates)
{
    var state = Ready();
    change(state);
    Check(!ListeningModePolicy.CanSetStrength(state, false), "Unknown, stale, unconfirmed, or pending state must not permit strength writes");
}
foreach (uint? allowOff in new uint?[] { null, 0, 2 })
{
    var state = Ready();
    if (allowOff.HasValue) state.DeviceControls[Feature.AllowOff] = allowOff.Value;
    else state.DeviceControls.Remove(Feature.AllowOff);
    Check(!ListeningModePolicy.CanSend(state, false, 0x0D, 1), "Off requires a report or a successfully sent choice");
}
var silentOff = Ready();
silentOff.DeviceControls.Remove(Feature.AllowOff);
silentOff.AllowOffRequested = true;
Check(ListeningModePolicy.CanSend(silentOff, false, 0x0D, 1), "A silent Allow Off request permits selecting Off");
var offGesture = new ListeningModeRequest();
offGesture.QueueGesture(1, 50);
Check(offGesture.TakeNext(silentOff) == (0x0D, 1u), "Dragging into Off uses the same effective choice");
offGesture.Cancel();
silentOff.AllowOffRequested = false;
offGesture.QueueGesture(1, 50);
Check(offGesture.TakeNext(silentOff) is null && !offGesture.HasTarget, "Disabling Allow Off cancels a queued Off gesture");
silentOff.AllowOffRequested = true;
silentOff.DeviceControlsPending[Feature.AllowOff] = 2;
Check(!ListeningModePolicy.CanSend(silentOff, false, 0x0D, 1), "A pending Allow Off change blocks an Off command");
var request = new ListeningModeRequest();
var device = Ready();
device.DeviceControls[0x0D] = 3;
request.QueueGesture(4, 75);
Check(request.TakeNext(device) == (0x0D, 4u), "Entering Adaptive sends its mode first");
Check(request.DisplayMode(3) == 4 && request.DisplayStrength(50) == 75, "Pending selection must stay where the user left it");
Check(device.DeviceControls[0x0D] == 3 && device.DeviceControls[ControlId.AdaptiveNoiseStrength] == 50, "Preview must never replace confirmed state");
request.Update(device, false);
Check(request.IsPending && request.TakeNext(device) is null, "Unrelated state before acknowledgement must not duplicate the request");
device.DeviceControlsPending[0x0D] = 4;
request.Update(device, false);
Check(request.IsPending && ListeningModePolicy.CanContinueGesture(device), "A pending request must not end the current pointer gesture");
device.DeviceControls[0x0D] = 4;
request.Update(device, false);
Check(request.TakeNext(device) is null, "A mode value still marked pending must not start strength early");
device.DeviceControlsPending.Clear();
request.Update(device, false);
Check(!request.IsPending && request.HasTarget, "Acknowledgement frees the sender while retaining the target");
Check(request.TakeNext(device) == (ControlId.AdaptiveNoiseStrength, 75u), "The next timer tick sends the queued strength after confirmation");
Check(request.TakeNext(device) is null && !request.Begin(device, 0x0D, 2), "Only one request may be outstanding");
device.AdaptiveStrengthRequested = 75;
request.Update(device, false);
Check(!request.IsPending && request.TakeNext(device) is null && !request.HasTarget, "Successful silent send completes the target without another write");

foreach (var failure in new[] { "disconnect", "stale", "error", "missing", "superseded", "rejected", "unconfirmed", "cancel" })
{
    device = Ready();
    device.DeviceControls[0x0D] = 3;
    request.QueueGesture(4, 25);
    Check(request.TakeNext(device) == (0x0D, 4u), "Each cancellation check starts with a queued strength");
    device.DeviceControlsPending[0x0D] = 4;
    request.Update(device, false);
    device.DeviceControlsPending.Clear();
    device.DeviceControls[0x0D] = 4;
    switch (failure)
    {
        case "disconnect": device.Connected = false; break;
        case "stale": device.LinkStale = true; break;
        case "missing": device.DeviceControls.Remove(0x0D); break;
        case "superseded": device.DeviceControlsPending[0x0D] = 2; break;
        case "rejected": device.DeviceControls[0x0D] = 3; break;
        case "unconfirmed": device.AncUnconfirmed = true; break;
        case "cancel": request.Cancel(); break;
    }
    request.Update(device, failure == "error");
    Check(!request.IsPending && !request.HasTarget, $"{failure} must discard queued intent");
    Check(request.TakeNext(Ready()) is null, $"{failure} must never replay on a later connection");
    Check(request.DisplayMode(3) == 3 && request.DisplayStrength(50) == 50, $"{failure} restores device visuals");
}

// Coalescing has one replaceable target, including after the pointer is released.
device = Ready();
request.QueueGesture(4, 1);
Check(request.TakeNext(device) == (ControlId.AdaptiveNoiseStrength, 1u), "First timer tick sends the current target");
for (uint value = 2; value <= 99; value++)
{
    request.QueueGesture(4, value);
    Check(request.TakeNext(device) is null, "Pointer changes must not fill the transport queue");
}
Check(request.DisplayStrength(50) == 99, "The thumb displays the latest pointer target while a prior send is pending");
device.AdaptiveStrengthRequested = 1;
request.Update(device, false);
Check(request.TakeNext(device) == (ControlId.AdaptiveNoiseStrength, 99u), "Release must retain and eventually flush only the final value");
device.AdaptiveStrengthRequested = 99;
request.Update(device, false);
Check(request.TakeNext(device) is null && !request.HasTarget, "No intermediate values replay after release");

// A late Adaptive acknowledgement must not replay a strength after the gesture leaves it.
device = Ready();
device.DeviceControls[0x0D] = 3;
request.QueueGesture(4, 25);
Check(request.TakeNext(device) == (0x0D, 4u), "Cross-mode drag starts with mode request");
request.QueueGesture(2, 25);
device.DeviceControls[0x0D] = 4;
request.Update(device, false);
Check(request.TakeNext(device) == (0x0D, 2u), "Latest target replaces the obsolete Adaptive strength");
device.DeviceControls[0x0D] = 2;
request.Update(device, false);
Check(request.TakeNext(device) is null && !request.HasTarget, "Leaving Adaptive must not leak a strength write");

// A missing strength setting or uncertain readback must not strand the mode selector.
device = Ready();
device.DeviceControls[0x0D] = 3;
device.DeviceControls.Remove(ControlId.AdaptiveNoiseStrength);
request.QueueGesture(4, 25);
Check(request.TakeNext(device) == (0x0D, 4u), "Missing strength readback must not prevent selecting Adaptive");
device.DeviceControls[0x0D] = 4;
request.Update(device, false);
Check(request.TakeNext(device) is null && !request.HasTarget, "Unavailable strength must not strand an otherwise completed mode gesture");
device = Ready();
device.AncUnconfirmed = true;
request.QueueGesture(4, 25);
Check(request.TakeNext(device) == (0x0D, 4u),
    "A fresh gesture must reassert an uncertain mode even if its old readback matches");
request.Update(device, false);
Check(request.IsPending, "Strength must wait until the retry confirms the mode");
device.AncUnconfirmed = false;
request.Update(device, false);
Check(request.TakeNext(device) == (ControlId.AdaptiveNoiseStrength, 25u),
    "The queued strength may follow the confirmed mode retry");
request.Cancel();

// Sent values stay distinct from old readback, especially when returning to that old value.
device = Ready();
Check(request.Begin(device, ControlId.AdaptiveNoiseStrength, 0), "Strength starts from reported 50");
device.AdaptiveStrengthRequested = 0;
request.Update(device, false);
Check(!request.IsPending, "Successful silent send releases the request without waiting for an echo");
Check(device.DeviceControls[ControlId.AdaptiveNoiseStrength] == 50, "Successful send must not replace device readback");
Check(DeviceSettingStatus.DisplayValue(device, ControlId.AdaptiveNoiseStrength) == 0, "Display the last successful request");
Check(!request.Begin(device, ControlId.AdaptiveNoiseStrength, 0), "An unchanged requested value needs no duplicate write");
Check(request.Begin(device, ControlId.AdaptiveNoiseStrength, 50), "Restoring old reported 50 after sent 0 must still send");
request.Update(device, false);
Check(request.IsPending && request.DisplayStrength(0) == 50, "Old reported 50 and sent 0 must keep the new 50 request pending");
device.AdaptiveStrengthRequested = 50;
request.Update(device, false);
Check(!request.IsPending, "The matching successful send releases the next request");
device.AdaptiveStrengthRequested = null;
device.DeviceControls[ControlId.AdaptiveNoiseStrength] = 100;
Check(DeviceSettingStatus.DisplayValue(device, ControlId.AdaptiveNoiseStrength) == 100, "A later device report replaces requested strength");

foreach (var model in new[] { "A3063", "A3047" })
foreach (var firmware in new[] { "9A348", "9A5347a", "9A349" })
for (uint value = 0; value <= 100; value++)
{
    device = Ready();
    device.Model = model;
    device.Firmware = firmware;
    device.DeviceControls[ControlId.AdaptiveNoiseStrength] = value == 50 ? 0u : 50u;
    request.QueueGesture(4, value);
    Check(request.TakeNext(device) == (ControlId.AdaptiveNoiseStrength, value), "All hardware must preserve every requested percentage");
    request.Cancel();
}
foreach (var firmware in new[] { "6619472", "90.3431000025000000.6807", "" })
{
    device = Ready();
    device.Firmware = firmware;
    request.QueueGesture(4, 25);
    Check(request.TakeNext(device) is null && !request.HasTarget,
        "Unrecognized firmware must not send an Adaptive gesture");
}
Check(!request.Begin(Ready(), ControlId.AdaptiveNoiseStrength, 101), "Invalid strength must never create pending state");
MotionChecks.Run();
Console.WriteLine("Listening selector geometry, continuous strength, gesture coalescing, failure handling, mode sequencing, and interruptible motion passed.");
