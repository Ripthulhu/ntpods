using System;
using System.Reflection;

namespace NTPods.WinUI.Services;

internal static class AppVersion
{
    public static string Read(Assembly assembly)
    {
        // Release builds keep the patch number in FileVersion.
        var value = assembly.GetCustomAttribute<AssemblyFileVersionAttribute>()?.Version;
        return Version.TryParse(value, out var version)
            ? $"v{version.Major}.{version.Minor}.{version.Build}"
            : "";
    }
}
