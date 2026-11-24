using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

internal static class DeviceSettingStatus
{
    public static uint? CurrentValue(byte id, uint raw)
    {
        // Hold reports include previous actions; only the current low word is editable.
        uint value = id == 0x16 ? raw & 0xFFFF : raw;
        bool valid = id switch
        {
            0x0A or 0x1B or 0x20 or 0x25 or 0x26 or 0x28 or 0x31 or 0x34 or 0x35 or 0x36 or 0x3B => value is 1 or 2,
            0x01 or 0x17 or 0x18 or 0x23 => value <= 2,
            0x0D => value is >= 1 and <= 4,
            0x16 => (value & 0xFF) is 1 or 5 && ((value >> 8) & 0xFF) is 1 or 5,
            0x1A => value <= 0x0F && System.Numerics.BitOperations.PopCount(value) >= 2,
            0x1C => value is 1 or 2,
            0x1F => value >> 8 == 0x50 && (value & 0xFF) <= 100,
            0x24 => value is 0x0200 or 0x0300,
            0x2E or 0x40 => value <= 100,
            _ => false
        };
        return valid ? value : null;
    }

    private static bool KnownValue(Snapshot s, byte id) =>
        s.DeviceControls.TryGetValue(id, out var raw) && CurrentValue(id, raw).HasValue;

    public static bool Available(Snapshot s, byte id) =>
        s.Connected && !s.LinkStale && DeviceModels.SupportsControl(s, id) && KnownValue(s, id);

    public static bool CanChange(Snapshot s, byte id) =>
        s.Connected && !s.LinkStale && DeviceModels.SupportsControl(s, id) && !s.DeviceControlsPending.ContainsKey(id) &&
        (s.DeviceControls.ContainsKey(id) ? KnownValue(s, id) :
            id == Feature.AllowOff && s.Model is "A3063" or "A3064" or "A3065");

    public static bool CanChoose(Snapshot s, byte id) =>
        CanEdit(s, id) && !s.DeviceControlsPending.ContainsKey(id);

    public static bool CanEdit(Snapshot s, byte id) =>
        s.Connected && !s.LinkStale && DeviceModels.SupportsControl(s, id) &&
        (s.DeviceControls.ContainsKey(id) ? KnownValue(s, id) :
            id == Feature.AllowOff && s.Model is "A3063" or "A3064" or "A3065" ||
            DeviceModels.Find(s.Model).Known && id is 0x01 or 0x0A or 0x16 or 0x1A or 0x23 or 0x31 or 0x40);

    public static uint ToggleValue(byte id, bool on) => id == 0x0A ? (on ? 2u : 1u) : (on ? 1u : 2u);

    public static bool AllowOffEnabled(Snapshot s) =>
        s.Connected && !s.LinkStale && DeviceModels.OffAvailable(s);

    public static uint? DisplayValue(Snapshot s, byte id) =>
        s.DeviceControls.TryGetValue(id, out var raw) && CurrentValue(id, raw) is null ? null :
        s.DeviceControlsPending.TryGetValue(id, out var requested) ? requested :
        id == Feature.AllowOff && s.AllowOffRequested is bool allowOff ? (allowOff ? 1u : 2u) :
        id == ControlId.AdaptiveNoiseStrength && s.AdaptiveStrengthRequested is uint sent ? sent :
        id == Feature.ConversationalAwareness && s.DeviceControlsRequested.TryGetValue(id, out var awareness) ? awareness :
        s.DeviceControls.TryGetValue(id, out var confirmed) ? CurrentValue(id, confirmed) :
        s.DeviceControlsRequested.TryGetValue(id, out var choice) ? choice : null;

    public static string Text(Snapshot s, byte id) => Localize.Get(
        !s.Connected ? "DeviceSetting.Disconnected" :
        s.LinkStale ? "DeviceSetting.Stale" :
        s.DeviceControls.TryGetValue(id, out var raw) && CurrentValue(id, raw) is null ? "DeviceSetting.Unknown" :
        s.DeviceControlsPending.ContainsKey(id) ? "DeviceSetting.Pending" :
        id == Feature.AllowOff && s.AllowOffRequested is not null ? "DeviceSetting.Sent" :
        id == ControlId.AdaptiveNoiseStrength && s.AdaptiveStrengthRequested is not null ? "DeviceSetting.Sent" :
        id == Feature.ConversationalAwareness && s.DeviceControlsRequested.ContainsKey(id) ? "DeviceSetting.Sent" :
        s.DeviceControls.ContainsKey(id) ? "DeviceSetting.Confirmed" :
        s.DeviceControlsRequested.ContainsKey(id) ? "DeviceSetting.Sent" : "DeviceSetting.Unknown");
}
