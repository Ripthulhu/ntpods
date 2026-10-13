using System;
using System.IO;
using Microsoft.Win32;

namespace NTPods.WinUI.Services;

/// Registers the daemon and tray app in the current user's Windows Run key.
public static class StartupService
{
    private const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string AppValue = "NTPods";        // the WinUI tray app
    private const string DaemonValue = "NTPods Daemon"; // the headless daemon

    public static (bool Enabled, bool HasEntries) ReadState()
    {
        using var key = Registry.CurrentUser.OpenSubKey(RunKey);
        return ReadState(key, Environment.ProcessPath, FindDaemon());
    }

    public static void SetEnabled(bool on)
    {
        using var key = Registry.CurrentUser.CreateSubKey(RunKey)
            ?? throw new IOException("The Windows startup registry key could not be opened.");
        SetEnabled(key, on, Environment.ProcessPath, FindDaemon());
    }

    internal static (bool Enabled, bool HasEntries) ReadState(RegistryKey? key, string? app, string? daemon)
    {
        var appCommand = key?.GetValue(AppValue);
        var daemonCommand = key?.GetValue(DaemonValue);
        return (File.Exists(app) && File.Exists(daemon) &&
            string.Equals(appCommand as string, $"\"{app}\" --tray", StringComparison.OrdinalIgnoreCase) &&
            string.Equals(daemonCommand as string, $"\"{daemon}\"", StringComparison.OrdinalIgnoreCase),
            appCommand is not null || daemonCommand is not null);
    }

    internal static void SetEnabled(RegistryKey key, bool on, string? app, string? daemon)
    {
        if (on)
        {
            // Check both binaries before writing either entry.
            if (!File.Exists(app) || !File.Exists(daemon))
                throw new FileNotFoundException("The NTPods app or background service is missing. Repair the installation first.");
            key.SetValue(AppValue, $"\"{app}\" --tray", RegistryValueKind.String);
            key.SetValue(DaemonValue, $"\"{daemon}\"", RegistryValueKind.String);
            if (!ReadState(key, app, daemon).Enabled)
                throw new IOException("Windows did not save both startup entries.");
        }
        else
        {
            key.DeleteValue(AppValue, throwOnMissingValue: false);
            key.DeleteValue(DaemonValue, throwOnMissingValue: false);
            if (key.GetValue(AppValue) is not null || key.GetValue(DaemonValue) is not null)
                throw new IOException("Windows did not remove both startup entries.");
        }
    }

    /// Keep search order aligned with DaemonClient.FindDaemon so startup uses the same binary.
    private static string? FindDaemon()
    {
        string[] candidates =
        {
            Path.Combine(AppContext.BaseDirectory, "ntpodsd.exe"),
            // MSI layout: ntpodsd.exe sits one folder up, next to winui\.
            Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "ntpodsd.exe")),
            Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
                "NTPods", "ntpodsd.exe"),
        };
        foreach (var c in candidates)
            if (File.Exists(c)) return c;
        return null;
    }
}
