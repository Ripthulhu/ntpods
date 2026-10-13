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

    /// Hide the card when the snapshot has no reported hosts.
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
            : Localize.Get(others == 1 ? "Multipoint_SharedOne" : "Multipoint_Shared", others);
    }
}
