namespace NTPods.WinUI.Services;

/// Code lookups use the current language; XAML binds to the same <see cref="Loc"/> instance.
internal static class Localize
{
    /// Look up a localized string by resw key (falls back to the key itself).
    public static string Get(string key) => Loc.Instance is { } loc ? loc.Get(key) : key;

    /// Look up a composite string and fill its <c>{0}</c>… placeholders.
    public static string Get(string key, params object[] args) =>
        Loc.Instance is { } loc ? loc.Get(key, args) : key;
}
