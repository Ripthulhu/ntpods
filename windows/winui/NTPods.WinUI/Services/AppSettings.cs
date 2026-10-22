using System.IO;
using System.Diagnostics;
using System.Text.Json;
using Microsoft.UI.Xaml;

namespace NTPods.WinUI.Services;

/// JSON settings at %LOCALAPPDATA%\NTPods\winui-settings.json for the unpackaged app.
/// Read failures use defaults; write failures leave the in-memory settings intact.
public static class AppSettings
{
    private sealed class Model
    {
        // 0 = System (Default), 1 = Light, 2 = Dark — the ThemeButtons indices.
        public int ThemeIndex { get; set; }
        public int? PendingThemeIndex { get; set; }
        public Dictionary<string, bool> ConnectionPopups { get; set; } = new(StringComparer.OrdinalIgnoreCase);
        public Dictionary<string, bool> DisconnectedBatteryCache { get; set; } = new(StringComparer.OrdinalIgnoreCase);

        // Hearing assistance and Custom Transparency are opt-in.
        public bool EnableExperimental { get; set; }

        // BCP-47 UI language override (e.g. "pt-PT"); "" = follow the system.
        public string LanguageTag { get; set; } = "";

        // Cache model 0x1D metadata for island artwork before the next session reports it.
        public string LastModel { get; set; } = "";
        public string LastDeviceId { get; set; } = "";
        public HotkeyGesture? MicHotkey { get; set; }
    }

    private static readonly object _gate = new();
    private static Model? _cache;

    private static string? Path_()
    {
        var local = System.Environment.GetFolderPath(System.Environment.SpecialFolder.LocalApplicationData);
        if (string.IsNullOrEmpty(local)) return null;
        return System.IO.Path.Combine(local, "NTPods", "winui-settings.json");
    }

    private static Model Load()
    {
        lock (_gate)
        {
            if (_cache is not null) return _cache;
            try
            {
                var path = Path_();
                if (path is not null && File.Exists(path))
                    _cache = JsonSerializer.Deserialize<Model>(File.ReadAllText(path));
            }
            catch { }
            return _cache ??= new Model();
        }
    }

    private static bool Update(Action<Model> update)
    {
        lock (_gate)
        {
            var model = Load();
            update(model);
            return SaveFile(Path_(), stream => JsonSerializer.Serialize(stream, model));
        }
    }

    internal static bool SaveFile(string? path, Action<FileStream> write)
    {
        try
        {
            if (path is null) throw new IOException("Local app data is unavailable.");
            Directory.CreateDirectory(System.IO.Path.GetDirectoryName(path)!);
            var temporary = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
            var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None);
            try
            {
                using (stream)
                {
                    write(stream);
                    stream.Flush(flushToDisk: true);
                }
                File.Move(temporary, path, overwrite: true);
            }
            finally
            {
                try { File.Delete(temporary); }
                catch (IOException) { }
                catch (UnauthorizedAccessException) { }
            }
            return true;
        }
        catch (Exception error)
        {
            // Exception messages can contain profile paths; retain only the error type and code.
            string message = $"App settings save failed: {error.GetType().Name} (0x{error.HResult:X8}).";
            Trace.TraceError(message);
            try
            {
                if (path is not null)
                    File.AppendAllText(System.IO.Path.Combine(System.IO.Path.GetDirectoryName(path)!, "winui.log"),
                        $"{DateTimeOffset.UtcNow:O} {message}{Environment.NewLine}");
            }
            catch { }
            return false;
        }
    }

    /// The saved theme selector index (0 System / 1 Light / 2 Dark).
    public static int ThemeIndex => Load().ThemeIndex;

    /// The saved theme as an ElementTheme for the window root.
    public static ElementTheme Theme => ThemeIndex switch
    {
        1 => ElementTheme.Light,
        2 => ElementTheme.Dark,
        _ => ElementTheme.Default,
    };

    /// Persist the chosen theme index.
    public static bool SetThemeIndex(int index) => Update(m => m.ThemeIndex = index);
    public static int? PendingThemeIndex => Load().PendingThemeIndex is >= 0 and <= 2 ? Load().PendingThemeIndex : null;
    public static bool SetLocalThemeIndex(int index) => index is >= 0 and <= 2 &&
        Update(m => { m.ThemeIndex = index; m.PendingThemeIndex = index; });
    public static bool AcknowledgeTheme(int index) => Update(m =>
    {
        m.ThemeIndex = index;
        if (m.PendingThemeIndex == index) m.PendingThemeIndex = null;
    });
    public static bool ConnectionPopupEnabled(string deviceId) => Load().ConnectionPopups is not { } popups ||
        !popups.TryGetValue(deviceId.Replace(":", "").ToUpperInvariant(), out bool enabled) || enabled;
    public static bool SetConnectionPopupEnabled(string deviceId, bool enabled) => Update(m =>
    {
        string key = deviceId.Replace(":", "").ToUpperInvariant();
        if (key.Length != 12) return;
        m.ConnectionPopups ??= new(StringComparer.OrdinalIgnoreCase);
        if (!m.ConnectionPopups.ContainsKey(key) && m.ConnectionPopups.Count >= 64)
            m.ConnectionPopups.Remove(m.ConnectionPopups.Keys.First());
        m.ConnectionPopups[key] = enabled;
    });
    public static bool CacheDisconnectedBattery(string deviceId) => Load().DisconnectedBatteryCache is not { } cache ||
        !cache.TryGetValue(deviceId.Replace(":", "").ToUpperInvariant(), out bool enabled) || enabled;
    public static bool SetCacheDisconnectedBattery(string deviceId, bool enabled) => Update(m =>
    {
        string key = deviceId.Replace(":", "").ToUpperInvariant();
        if (key.Length != 12) return;
        m.DisconnectedBatteryCache ??= new(StringComparer.OrdinalIgnoreCase);
        if (!m.DisconnectedBatteryCache.ContainsKey(key) && m.DisconnectedBatteryCache.Count >= 64)
            m.DisconnectedBatteryCache.Remove(m.DisconnectedBatteryCache.Keys.First());
        m.DisconnectedBatteryCache[key] = enabled;
    });

    /// Whether experimental settings are shown (off by default).
    public static bool EnableExperimental => Load().EnableExperimental;

    /// Persist the experimental-features opt-in.
    public static bool SetEnableExperimental(bool on) => Update(m => m.EnableExperimental = on);

    /// The last-seen device model number (for the connect island's early artwork).
    public static string LastModel => Load().LastModel ?? "";
    public static string LastDeviceId => Load().LastDeviceId ?? "";

    /// Persist the last-seen device model number.
    public static bool SetLastModel(string model, string deviceId)
    {
        if (string.IsNullOrWhiteSpace(model)) return true;
        lock (_gate)
        {
            var m = Load();
            if (m.LastModel == model && m.LastDeviceId == deviceId) return true;
            return Update(m => { m.LastModel = model; m.LastDeviceId = deviceId; });
        }
    }

    /// The saved UI language override (BCP-47, e.g. "pt-PT"); "" = follow the system.
    public static string LanguageTag => Load().LanguageTag ?? "";

    public static HotkeyGesture? MicHotkey => Load().MicHotkey is { IsValid: true } gesture ? gesture : null;

    public static bool SetMicHotkey(HotkeyGesture? gesture) =>
        Update(m => m.MicHotkey = gesture is { IsValid: true } ? gesture : null);

    /// Persist the language choice; the caller updates Loc for the current session.
    public static bool SetLanguageTag(string tag) => Update(m => m.LanguageTag = tag ?? "");
}
