namespace NTPods.WinUI.Services;

internal static class Localize
{
    public static string Get(string key) => key;
    public static string Get(string key, params object[] args) => key + " " + string.Join(" ", args);
}
