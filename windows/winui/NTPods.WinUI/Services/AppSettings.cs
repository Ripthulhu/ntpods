using System.IO;
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

    private static void Save(Model m)
    {
        lock (_gate)
        {
            _cache = m;
            try
            {
                var path = Path_();
                if (path is null) return;
                var dir = System.IO.Path.GetDirectoryName(path);
                if (!string.IsNullOrEmpty(dir)) Directory.CreateDirectory(dir);
                File.WriteAllText(path, JsonSerializer.Serialize(m));
            }
            catch { }
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
    public static void SetThemeIndex(int index)
    {
        var m = Load();
        m.ThemeIndex = index;
        Save(m);
    }

    /// Whether experimental settings are shown (off by default).
    public static bool EnableExperimental => Load().EnableExperimental;

    /// Persist the experimental-features opt-in.
    public static void SetEnableExperimental(bool on)
    {
        var m = Load();
        m.EnableExperimental = on;
        Save(m);
    }

    /// The last-seen device model number (for the connect island's early artwork).
    public static string LastModel => Load().LastModel ?? "";
    public static string LastDeviceId => Load().LastDeviceId ?? "";

    /// Persist the last-seen device model number.
    public static void SetLastModel(string model, string deviceId)
    {
        if (string.IsNullOrWhiteSpace(model)) return;
        var m = Load();
        if (m.LastModel == model && m.LastDeviceId == deviceId) return;
        m.LastModel = model;
        m.LastDeviceId = deviceId;
        Save(m);
    }

    /// The saved UI language override (BCP-47, e.g. "pt-PT"); "" = follow the system.
    public static string LanguageTag => Load().LanguageTag ?? "";

    public static HotkeyGesture? MicHotkey => Load().MicHotkey is { IsValid: true } gesture ? gesture : null;

    public static void SetMicHotkey(HotkeyGesture? gesture)
    {
        var m = Load();
        m.MicHotkey = gesture is { IsValid: true } ? gesture : null;
        Save(m);
    }

    /// Persist the language choice; the caller updates Loc for the current session.
    public static void SetLanguageTag(string tag)
    {
        var m = Load();
        m.LanguageTag = tag ?? "";
        Save(m);
    }
}
