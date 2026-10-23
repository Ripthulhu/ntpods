namespace NTPods.WinUI.Services;

internal static class Localize
{
    public static string Get(string key) => key;
    public static string Get(string key, params object[] args) => key + " " + string.Join(" ", args);
}

// Layout checks use the default cache policy; DeviceSettings covers both policies.
internal static class AppSettings
{
    public static bool CacheDisconnectedBattery(string _) => true;
}
