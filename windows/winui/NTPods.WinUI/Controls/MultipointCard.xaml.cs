using System.Collections.Generic;
using System.Linq;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace NTPods.WinUI.Controls;

/// Shows hosts reported by the AirPods in AAP 0x2E. Windows only reports its own
/// link and can remain connected while another host receives the audio.
public sealed partial class MultipointCard : UserControl
{
    public MultipointCard()
    {
        InitializeComponent();
    }

    /// Render the device list from a daemon Snapshot. The card hides itself until
    /// the first 0x2E arrives, so it never shows an empty shell.
    public void Update(Snapshot s)
    {
        List<ConnectedDevice> devices = s.Multipoint ?? new();
        if (devices.Count == 0)
        {
            Visibility = Visibility.Collapsed;
            return;
        }

        Visibility = Visibility.Visible;
        DeviceList.ItemsSource = devices;

        int others = devices.Count(d => !d.IsThisPc);
        SummaryText.Text = others == 0
            ? Localize.Get("Multipoint_OnlyThisPc")
            : string.Format(Localize.Get("Multipoint_Shared"), others);
    }
}
