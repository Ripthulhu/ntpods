using System.Runtime.InteropServices;
using NTPods.WinUI.Services;

static void Check(bool value, string message)
{
    if (!value) throw new Exception(message);
}

uint ctrl = HotkeyGesture.Control, alt = HotkeyGesture.Alt, shift = HotkeyGesture.Shift;
Check(new HotkeyGesture(ctrl | alt, 'M').IsValid, "Ctrl+Alt+M must be accepted.");
Check(new HotkeyGesture(ctrl | shift, 0x86).IsValid, "Ctrl+Shift+F23 must be accepted.");
Check(!new HotkeyGesture(ctrl | alt, 0x09).IsValid, "Tab must remain available for dialog navigation.");
foreach (uint modifiers in new uint[] { 0, shift, HotkeyGesture.Windows, ctrl | HotkeyGesture.Windows, 0x4002, uint.MaxValue })
    Check(!new HotkeyGesture(modifiers, 'M').IsValid, "Bare, Shift-only, Win and unknown modifier flags must be rejected.");
foreach (uint key in new uint[] { 0, 1, 0x10, 0x11, 0x12, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C, 0x7B, 0xFF, uint.MaxValue })
    Check(!new HotkeyGesture(ctrl, key).IsValid, "Reserved, modifier-only and invalid keys must be rejected.");
Check(HotkeyGesture.ModifierForKey(0x11) == ctrl && HotkeyGesture.ModifierForKey(0xA2) == ctrl
    && HotkeyGesture.ModifierForKey(0xA3) == ctrl, "Both Ctrl keys must normalize.");
Check(HotkeyGesture.ModifierForKey(0x12) == alt && HotkeyGesture.ModifierForKey(0xA4) == alt
    && HotkeyGesture.ModifierForKey(0xA5) == alt, "Both Alt keys must normalize.");
Check(HotkeyGesture.ModifierForKey(0x10) == shift && HotkeyGesture.ModifierForKey(0xA0) == shift
    && HotkeyGesture.ModifierForKey(0xA1) == shift, "Both Shift keys must normalize.");
Check(HotkeyGesture.IsModifierKey(0x5B) && HotkeyGesture.IsModifierKey(0x5C)
    && !HotkeyGesture.IsModifierKey('M'), "Windows keys must be recognized as modifiers.");
Check(new HotkeyGesture(ctrl | alt | shift, 'M').ToString() == "Ctrl + Alt + Shift + M", "Modifier order must be stable.");
Check(new HotkeyGesture(ctrl, 0x87).ToString() == "Ctrl + F24", "Function-key names must preserve their number.");
Console.WriteLine("Hotkey gestures: validation, modifier normalization and display passed.");

if (!args.Contains("--native")) return;
Check(OperatingSystem.IsWindows(), "Native hotkey tests require Windows.");

// Obscure shortcuts are reserved only for this test; no keyboard input is injected.
var first = new HotkeyGesture(ctrl | alt | shift, 0x86);
var second = new HotkeyGesture(ctrl | alt | shift, 0x87);
nint window = Native.CreateWindowEx(0, "STATIC", "", 0, 0, 0, 0, 0, 0, 0, 0, 0);
nint otherWindow = Native.CreateWindowEx(0, "STATIC", "", 0, 0, 0, 0, 0, 0, 0, 0, 0);
Check(window != 0 && otherWindow != 0, "Hidden test windows must be created.");
try
{
    using var hotkey = new GlobalHotkey(window);
    using var other = new GlobalHotkey(otherWindow);
    Check(hotkey.TrySet(first, out int error), $"Could not reserve the first test shortcut (Windows error {error}).");
    Check(other.TrySet(second, out error), $"Could not reserve the second test shortcut (Windows error {error}).");
    int pressed = 0, captured = 0;
    hotkey.Pressed += () => pressed++;
    HotkeyGesture? capturedGesture = null;
    hotkey.Captured += gesture => { capturedGesture = gesture; captured++; };

    Check(!hotkey.TrySet(second, out error) && error == 1409, "An occupied shortcut must report ERROR_HOTKEY_ALREADY_REGISTERED.");
    Check(hotkey.Current == first, "A registration conflict must preserve the original shortcut.");
    Native.Deliver(window, 0x4E50, first);
    Check(pressed == 1, "The original shortcut must still dispatch after a conflict.");

    hotkey.IsSuspended = true;
    Native.Deliver(window, 0x4E50, first);
    Check(pressed == 1 && captured == 1, "Shortcut capture must not activate the microphone.");
    Check(capturedGesture == first, "Capture must keep the registered gesture.");
    hotkey.IsSuspended = false;
    Check(other.TrySet(null, out error), "Clearing the competing shortcut must succeed.");
    Check(hotkey.TrySet(second, out error) && hotkey.Current == second, "Replacement must succeed after the conflict is cleared.");
    Native.Deliver(window, 0x4E50, first);
    Check(pressed == 1, "Queued messages for the removed shortcut must be ignored.");
    Native.Deliver(window, 0x4E51, second);
    Check(pressed == 2, "The replacement shortcut must dispatch.");
    Check(other.TrySet(first, out error), "The old shortcut must be available after replacement.");
    Check(hotkey.TrySet(second, out error), "Setting the current shortcut again must be harmless.");
    Check(!hotkey.TrySet(new HotkeyGesture(0, 'M'), out error) && error == 87 && hotkey.Current == second,
        "Invalid replacements must preserve the current shortcut.");
    Action failedPress = () => throw new InvalidOperationException("Test listener failure.");
    Action<HotkeyGesture> failedCapture = _ => throw new InvalidOperationException("Test capture failure.");
    hotkey.Pressed += failedPress;
    Native.Deliver(window, 0x4E51, second);
    hotkey.Pressed -= failedPress;
    Native.Deliver(window, 0x4E51, second);
    Check(pressed == 4, "A failed listener must not escape the native callback or break later shortcuts.");
    hotkey.IsSuspended = true;
    hotkey.Captured += failedCapture;
    Native.Deliver(window, 0x4E51, second);
    hotkey.Captured -= failedCapture;
    Native.Deliver(window, 0x4E51, second);
    Check(captured == 3 && capturedGesture == second && pressed == 4,
        "A failed capture listener must not escape the callback or enable the microphone.");
    hotkey.Dispose();
    Check(other.TrySet(second, out error), "Disposing must release the registration.");
    Native.DestroyWindow(otherWindow);
    otherWindow = 0;
    Check(other.Current is null, "Window destruction must clear the registration and subclass.");
    using var afterDestroy = new GlobalHotkey(window);
    Check(afterDestroy.TrySet(second, out error), "Window destruction must release the shortcut.");
    Console.WriteLine("Native hotkeys: conflict preservation, capture suppression, replacement, stale messages, listener failures and cleanup passed.");
}
finally
{
    if (otherWindow != 0) Native.DestroyWindow(otherWindow);
    Native.DestroyWindow(window);
}

static class Native
{
    public static void Deliver(nint window, nuint id, HotkeyGesture gesture) =>
        SendMessage(window, 0x0312, id, (nint)((gesture.VirtualKey << 16) | gesture.Modifiers));

    [DllImport("user32.dll", EntryPoint = "CreateWindowExW", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern nint CreateWindowEx(uint extendedStyle, string className, string name, uint style,
        int x, int y, int width, int height, nint parent, nint menu, nint instance, nint parameter);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool DestroyWindow(nint window);

    [DllImport("user32.dll", EntryPoint = "SendMessageW")]
    private static extern nint SendMessage(nint window, uint message, nuint wParam, nint lParam);
}
