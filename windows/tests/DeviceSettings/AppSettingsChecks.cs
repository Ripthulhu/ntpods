using System.Diagnostics;
using System.Text;
using NTPods.WinUI.Services;

internal static class AppSettingsChecks
{
    public static bool RunInterruptedWriter(string[] args)
    {
        if (args.Length != 3 || args[0] != "--interrupt-settings-write") return false;
        AppSettings.SaveFile(args[1], stream =>
        {
            stream.Write(Encoding.UTF8.GetBytes("partial"));
            stream.Flush(flushToDisk: true);
            File.WriteAllText(args[2], "ready");
            Thread.Sleep(Timeout.Infinite);
        });
        return true;
    }

    public static void Run(Action<bool, string> check)
    {
        string directory = Path.Combine(Path.GetTempPath(), "ntpods-settings-" + Guid.NewGuid().ToString("N"));
        string path = Path.Combine(directory, "settings.json");
        bool Save(string value) => AppSettings.SaveFile(path, stream => stream.Write(Encoding.UTF8.GetBytes(value)));
        const string previous = "{\"ThemeIndex\":1,\"LanguageTag\":\"fr-FR\"}";
        const string replacement = "{\"ThemeIndex\":2,\"LanguageTag\":\"pt-PT\"}";
        try
        {
            check(Save(previous), "A first save must create its settings directory.");
            check(!AppSettings.SaveFile(path, stream =>
            {
                stream.Write(Encoding.UTF8.GetBytes("partial"));
                throw new IOException("simulated private device identity");
            }), "A failed write must report failure.");
            check(File.ReadAllText(path) == previous, "A failed write must retain the previous complete settings.");
            check(Directory.GetFiles(directory, "*.tmp").Length == 0, "A failed write must remove its temporary file.");
            string log = File.ReadAllText(Path.Combine(directory, "winui.log"));
            check(log.Contains("IOException") && !log.Contains("private device identity"),
                "Save logs must retain the error type without copying sensitive exception messages.");

            using (var locked = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read))
            {
                check(!Save(replacement), "A blocked replacement must report failure.");
                check(File.ReadAllText(path) == previous, "A blocked replacement must retain the previous settings.");
                check(Directory.GetFiles(directory, "*.tmp").Length == 0, "A blocked replacement must remove its temporary file.");
            }

            var start = new ProcessStartInfo(Environment.ProcessPath!) { UseShellExecute = false, CreateNoWindow = true };
            if (string.Equals(Path.GetFileNameWithoutExtension(Environment.ProcessPath), "dotnet", StringComparison.OrdinalIgnoreCase))
                start.ArgumentList.Add(typeof(AppSettingsChecks).Assembly.Location);
            string ready = Path.Combine(directory, "writer-ready");
            start.ArgumentList.Add("--interrupt-settings-write");
            start.ArgumentList.Add(path);
            start.ArgumentList.Add(ready);
            using (var child = Process.Start(start)!)
            {
                try
                {
                    check(SpinWait.SpinUntil(() => File.Exists(ready) || child.HasExited, TimeSpan.FromSeconds(10)) && File.Exists(ready),
                        "The child must reach a flushed partial write before interruption.");
                    child.Kill(entireProcessTree: true);
                    check(child.WaitForExit(5000), "The interrupted settings writer must exit.");
                }
                finally
                {
                    if (!child.HasExited) { child.Kill(entireProcessTree: true); child.WaitForExit(); }
                }
            }
            check(File.ReadAllText(path) == previous, "Process interruption must preserve the complete previous settings.");
            string[] abandoned = Directory.GetFiles(directory, "*.tmp");
            check(abandoned.Length == 1, "Interruption leaves only the unfinished temporary file.");
            check(Save(replacement) && File.ReadAllText(path) == replacement,
                "A later save must succeed despite an abandoned temporary file.");
            check(File.Exists(abandoned[0]), "A successful save must not remove another writer's temporary file.");
            check(!AppSettings.SaveFile(null, _ => throw new InvalidOperationException()),
                "An unavailable profile must report a save failure.");
        }
        finally
        {
            if (Directory.Exists(directory))
            {
                foreach (string file in Directory.GetFiles(directory)) File.Delete(file);
                Directory.Delete(directory);
            }
        }
        Console.WriteLine("Atomic app settings and interrupted-write checks passed.");
    }
}
