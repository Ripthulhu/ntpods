using System.Runtime.InteropServices;
using System.Text;

namespace NTPods.WinUI.Services;

public readonly record struct HotkeyGesture(uint Modifiers, uint VirtualKey)
{
    public const uint Alt = 1;
    public const uint Control = 2;
    public const uint Shift = 4;
    public const uint Windows = 8;

    public bool IsValid => (Modifiers & (Alt | Control)) != 0
        && (Modifiers & ~(Alt | Control | Shift)) == 0
        && VirtualKey != 0x7B // F12 is reserved for the Windows debugger.
        && VirtualKey is 0x08 or 0x0D or 0x20
            or >= 0x21 and <= 0x28 or 0x2D or 0x2E
            or >= 0x30 and <= 0x39 or >= 0x41 and <= 0x5A
            or >= 0x60 and <= 0x87
            or >= 0xBA and <= 0xC0 or >= 0xDB and <= 0xDF or 0xE2;

    public static uint ModifierForKey(uint key) => key switch
    {
        0x10 or 0xA0 or 0xA1 => Shift,
        0x11 or 0xA2 or 0xA3 => Control,
        0x12 or 0xA4 or 0xA5 => Alt,
        0x5B or 0x5C => Windows,
        _ => 0,
    };

    public static bool IsModifierKey(uint key) => ModifierForKey(key) != 0;

    public override string ToString()
    {
        var parts = new List<string>(4);
        if ((Modifiers & Control) != 0) parts.Add("Ctrl");
        if ((Modifiers & Alt) != 0) parts.Add("Alt");
        if ((Modifiers & Shift) != 0) parts.Add("Shift");
        parts.Add(KeyName());
        return string.Join(" + ", parts);
    }

    private string KeyName()
    {
        if (VirtualKey is >= 0x30 and <= 0x39 or >= 0x41 and <= 0x5A)
            return ((char)VirtualKey).ToString();
        if (VirtualKey is >= 0x70 and <= 0x87) return $"F{VirtualKey - 0x6F}";

        uint scanCode = MapVirtualKey(VirtualKey, 0);
        if (VirtualKey is >= 0x21 and <= 0x28 or 0x2D or 0x2E or 0x6F)
            scanCode |= 0x100;
        var name = new StringBuilder(64);
        return GetKeyNameText(unchecked((int)(scanCode << 16)), name, name.Capacity) > 0
            ? name.ToString() : $"0x{VirtualKey:X2}";
    }

    [DllImport("user32.dll", EntryPoint = "MapVirtualKeyW")]
    private static extern uint MapVirtualKey(uint code, uint mapType);

    [DllImport("user32.dll", EntryPoint = "GetKeyNameTextW", CharSet = CharSet.Unicode)]
    private static extern int GetKeyNameText(int lParam, StringBuilder text, int size);
}
