using System.Runtime.InteropServices;

namespace NTPods.WinUI.Services;

/// Create, update and dispose on the window's UI thread.
public sealed class GlobalHotkey : IDisposable
{
    private const uint WmHotkey = 0x0312;
    private const uint WmNcDestroy = 0x0082;
    private const uint NoRepeat = 0x4000;
    private const int FirstId = 0x4E50;
    private const int SecondId = FirstId + 1;
    private const nuint SubclassId = 0x4E54504D;
    private readonly SubclassProc _callback;
    private readonly int _threadId = Environment.CurrentManagedThreadId;
    private nint _window;
    private int _registeredId;

    public GlobalHotkey(nint window)
    {
        if (window == 0) throw new ArgumentException("A window handle is required.", nameof(window));
        _callback = WindowProcedure;
        if (!SetWindowSubclass(window, _callback, SubclassId, 0))
            throw new InvalidOperationException("Could not listen for keyboard shortcuts.");
        _window = window;
    }

    public HotkeyGesture? Current { get; private set; }
    public bool IsSuspended { get; set; }
    public event Action? Pressed;
    public event Action<HotkeyGesture>? Captured;

    public bool TrySet(HotkeyGesture? gesture, out int errorCode)
    {
        CheckThread();
        ObjectDisposedException.ThrowIf(_window == 0, this);
        errorCode = 0;
        if (gesture is { IsValid: false })
        {
            errorCode = 87; // ERROR_INVALID_PARAMETER
            return false;
        }
        if (gesture == Current) return true;

        int nextId = _registeredId == FirstId ? SecondId : FirstId;
        if (gesture is { } next && !RegisterHotKey(_window, nextId, next.Modifiers | NoRepeat, next.VirtualKey))
        {
            errorCode = Marshal.GetLastWin32Error();
            return false;
        }

        // Reserve the replacement first so a conflict leaves the old shortcut working.
        if (Current is not null && !UnregisterHotKey(_window, _registeredId))
        {
            errorCode = Marshal.GetLastWin32Error();
            if (gesture is not null) UnregisterHotKey(_window, nextId);
            return false;
        }
        Current = gesture;
        _registeredId = gesture is null ? 0 : nextId;
        return true;
    }

    private nint WindowProcedure(nint window, uint message, nuint wParam, nint lParam, nuint id, nuint data)
    {
        if (message == WmHotkey && (int)wParam == _registeredId && Current is { } gesture)
        {
            uint packed = unchecked((uint)lParam);
            if ((packed & 0xFFFF) == gesture.Modifiers && packed >> 16 == gesture.VirtualKey)
            {
                try
                {
                    if (IsSuspended) Captured?.Invoke(gesture);
                    else Pressed?.Invoke();
                }
                catch (Exception error)
                {
                    // Managed listeners must not unwind into the native window procedure.
                    System.Diagnostics.Debug.WriteLine(error);
                }
            }
            return 0;
        }
        if (message == WmNcDestroy) Dispose();
        return DefSubclassProc(window, message, wParam, lParam);
    }

    public void Dispose()
    {
        CheckThread();
        if (_window == 0) return;
        if (Current is not null) UnregisterHotKey(_window, _registeredId);
        RemoveWindowSubclass(_window, _callback, SubclassId);
        _window = 0;
        Current = null;
        _registeredId = 0;
        Pressed = null;
        Captured = null;
    }

    private void CheckThread()
    {
        if (Environment.CurrentManagedThreadId != _threadId)
            throw new InvalidOperationException("Keyboard shortcuts must be changed on the window's UI thread.");
    }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate nint SubclassProc(nint window, uint message, nuint wParam, nint lParam, nuint id, nuint data);

    [DllImport("comctl32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowSubclass(nint window, SubclassProc callback, nuint id, nuint data);

    [DllImport("comctl32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool RemoveWindowSubclass(nint window, SubclassProc callback, nuint id);

    [DllImport("comctl32.dll")]
    private static extern nint DefSubclassProc(nint window, uint message, nuint wParam, nint lParam);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool RegisterHotKey(nint window, int id, uint modifiers, uint key);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool UnregisterHotKey(nint window, int id);
}
