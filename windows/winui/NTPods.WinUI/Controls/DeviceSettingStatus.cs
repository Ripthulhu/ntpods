using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

internal static class DeviceSettingStatus
{
    public static bool Available(Snapshot s, byte id) =>
        s.Connected && !s.LinkStale && DeviceModels.SupportsControl(s, id) && s.DeviceControls.ContainsKey(id);

    public static bool CanChange(Snapshot s, byte id) =>
        s.Connected && !s.LinkStale && DeviceModels.SupportsControl(s, id) && !s.DeviceControlsPending.ContainsKey(id) &&
        (s.DeviceControls.ContainsKey(id) ||
            id == Feature.AllowOff && s.Model is "A3063" or "A3064" or "A3065");

    public static bool AllowOffEnabled(Snapshot s) =>
        s.Connected && !s.LinkStale && DeviceModels.OffAvailable(s);

    public static uint? DisplayValue(Snapshot s, byte id) =>
        s.DeviceControlsPending.TryGetValue(id, out var requested) ? requested :
        id == Feature.AllowOff && s.AllowOffRequested is bool allowOff ? (allowOff ? 1u : 2u) :
        id == ControlId.AdaptiveNoiseStrength && s.AdaptiveStrengthRequested is uint sent ? sent :
        s.DeviceControls.TryGetValue(id, out var confirmed) ? confirmed : null;

    public static string Text(Snapshot s, byte id) => Localize.Get(
        !s.Connected ? "DeviceSetting.Disconnected" :
        s.LinkStale ? "DeviceSetting.Stale" :
        s.DeviceControlsPending.ContainsKey(id) ? "DeviceSetting.Pending" :
        id == Feature.AllowOff && s.AllowOffRequested is not null ? "DeviceSetting.Sent" :
        id == ControlId.AdaptiveNoiseStrength && s.AdaptiveStrengthRequested is not null ? "DeviceSetting.Sent" :
        s.DeviceControls.ContainsKey(id) ? "DeviceSetting.Confirmed" : "DeviceSetting.Unknown");
}
