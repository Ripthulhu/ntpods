using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;

namespace NTPods.WinUI.Popup;

internal static class PopupWindowNative
{
    public static void Configure(Window window, bool interactive)
    {
        window.AppWindow.IsShownInSwitchers = false;
        if (window.AppWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.SetBorderAndTitleBar(false, false);
            presenter.IsAlwaysOnTop = true;
            presenter.IsResizable = false;
            presenter.IsMaximizable = false;
            presenter.IsMinimizable = false;
        }
        nint hwnd = WinRT.Interop.WindowNative.GetWindowHandle(window);
        nint extended = (GetWindowLongPtr(hwnd, -20) & ~(nint)0x08020301) | 0x00000080;
        SetWindowLongPtr(hwnd, -20, interactive ? extended : extended | 0x08000000);
        // These windows own their rounded edge and transition, without a DWM frame.
        SetWindowLongPtr(hwnd, -16, (GetWindowLongPtr(hwnd, -16) & ~(nint)0x00CF0000) | unchecked((nint)0x80000000));
        int disabled = 1;
        DwmSetWindowAttribute(hwnd, 2, ref disabled, sizeof(int));
        DwmSetWindowAttribute(hwnd, 3, ref disabled, sizeof(int));
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22000))
        {
            int noBorder = unchecked((int)0xFFFFFFFE);
            DwmSetWindowAttribute(hwnd, 34, ref noBorder, sizeof(int));
            DwmSetWindowAttribute(hwnd, 33, ref disabled, sizeof(int));
        }
        SetWindowPos(hwnd, 0, 0, 0, 0, 0, 0x0037);
    }

    public static double Scale(Window window) =>
        Math.Max(1, GetDpiForWindow(WinRT.Interop.WindowNative.GetWindowHandle(window)) / 96.0);

    public static void SetRegion(Window window, int left, int top, int width, int height, int diameter)
    {
        nint region = CreateRoundRectRgn(left, top, left + width + 1, top + height + 1, diameter, diameter);
        if (region == 0) throw new Win32Exception();
        if (SetWindowRgn(WinRT.Interop.WindowNative.GetWindowHandle(window), region, true) != 0) return;
        DeleteObject(region);
        throw new Win32Exception();
    }

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW", SetLastError = true)]
    private static extern nint GetWindowLongPtr(nint hwnd, int index);
    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW", SetLastError = true)]
    private static extern nint SetWindowLongPtr(nint hwnd, int index, nint value);
    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(nint hwnd);
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowPos(nint hwnd, nint after, int x, int y, int width, int height, uint flags);
    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(nint hwnd, uint attribute, ref int value, int size);
    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern nint CreateRoundRectRgn(int left, int top, int right, int bottom, int ellipseWidth, int ellipseHeight);
    [DllImport("user32.dll", SetLastError = true)]
    private static extern int SetWindowRgn(nint hwnd, nint region, [MarshalAs(UnmanagedType.Bool)] bool redraw);
    [DllImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DeleteObject(nint obj);
}
