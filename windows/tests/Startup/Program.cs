using Microsoft.Win32;
using NTPods.WinUI.Services;

static void Check(bool condition, string message)
{
    if (!condition) throw new InvalidOperationException(message);
}

static void MustFail(Action action)
{
    try { action(); }
    catch (Exception) { return; }
    throw new InvalidOperationException("A failed startup change must not report success.");
}

// Use a private test key; never write the user's Run key.
var id = Guid.NewGuid().ToString("N");
var keyPath = @"Software\NTPods\StartupTests\" + id;
var directory = Path.Combine(Path.GetTempPath(), "ntpods-startup-test-" + id);
Directory.CreateDirectory(directory);
var app = Path.Combine(directory, "app.exe");
var daemon = Path.Combine(directory, "daemon.exe");
File.WriteAllText(app, "fixture");
File.WriteAllText(daemon, "fixture");
try
{
    using var key = Registry.CurrentUser.CreateSubKey(keyPath);
    Check(!StartupService.ReadState(key, app, daemon).Enabled, "Empty startup key is off.");
    Check(!StartupService.ReadState(key, app, daemon).HasEntries, "Empty startup key must not show a cleanup warning.");
    key.SetValue("NTPods", $"\"{app}\" --tray");
    Check(!StartupService.ReadState(key, app, daemon).Enabled, "App-only startup is incomplete.");
    Check(StartupService.ReadState(key, app, daemon).HasEntries, "App-only startup must expose its remaining entry for cleanup.");
    key.DeleteValue("NTPods");
    key.SetValue("NTPods Daemon", $"\"{daemon}\"");
    Check(!StartupService.ReadState(key, app, daemon).Enabled, "Daemon-only startup is incomplete.");
    Check(StartupService.ReadState(key, app, daemon).HasEntries, "Daemon-only startup must expose its remaining entry for cleanup.");

    StartupService.SetEnabled(key, true, app, daemon);
    Check(StartupService.ReadState(key, app, daemon).Enabled, "Both startup commands must roundtrip.");
    Check((string?)key.GetValue("NTPods") == $"\"{app}\" --tray", "Tray argument and path quoting changed.");
    key.SetValue("NTPods", "\"C:\\old-install\\app.exe\" --tray");
    Check(!StartupService.ReadState(key, app, daemon).Enabled, "A stale binary path is not enabled.");
    StartupService.SetEnabled(key, true, app, daemon);
    using (var readOnly = Registry.CurrentUser.OpenSubKey(keyPath, writable: false)!)
        MustFail(() => StartupService.SetEnabled(readOnly, false, app, daemon));
    Check(StartupService.ReadState(key, app, daemon).Enabled, "A denied write must leave the real state observable.");

    StartupService.SetEnabled(key, false, app, daemon);
    Check(key.GetValue("NTPods") is null && key.GetValue("NTPods Daemon") is null,
        "Turning startup off must remove both entries.");
    MustFail(() => StartupService.SetEnabled(key, true, app, Path.Combine(directory, "missing.exe")));
    Check(key.GetValue("NTPods") is null && key.GetValue("NTPods Daemon") is null,
        "A missing daemon must not register the app alone.");
    StartupService.SetEnabled(key, false, null, null);
    using var closed = Registry.CurrentUser.OpenSubKey(keyPath)!;
    closed.Close();
    MustFail(() => StartupService.ReadState(closed, app, daemon));
    Console.WriteLine("Startup checks passed: paired commands, stale paths, missing files, denied writes and removal.");
}
finally
{
    Registry.CurrentUser.DeleteSubKeyTree(keyPath, throwOnMissingSubKey: false);
    Check(Path.GetDirectoryName(Path.GetFullPath(directory)) == Path.GetFullPath(Path.GetTempPath()).TrimEnd(Path.DirectorySeparatorChar),
        "Unsafe temporary directory cleanup.");
    Directory.Delete(directory, recursive: true);
}
