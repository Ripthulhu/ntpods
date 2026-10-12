using System.Text;
using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class RenameRequest
{
    private ulong _next;
    public ulong? Pending { get; private set; }
    public static bool Valid(string name) => !string.IsNullOrWhiteSpace(name) && Encoding.UTF8.GetByteCount(name) <= 64;
    public ulong Begin() => (Pending = ++_next).Value;
    public void Cancel() => Pending = null;
    public bool Complete(DaemonEvent.CommandResult result)
    {
        if (result.Command != "set_name" || Pending != result.RequestId) return false;
        Pending = null;
        return true;
    }
}
