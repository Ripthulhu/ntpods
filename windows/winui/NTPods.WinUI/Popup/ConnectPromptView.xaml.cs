using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Popup;

public sealed partial class ConnectPromptView : UserControl
{
    public event Action? ConnectRequested;
    public event Action? DismissRequested;

    public ConnectPromptView()
    {
        InitializeComponent();
        // Discovery has no verified model yet; this is the shared artwork fallback.
        DeviceImage.Source = new BitmapImage(new Uri(DeviceArt.MainImage(null)));
    }

    public void SetName(string name) => DeviceName.Text = name;
    public void Fit(double height)
    {
        CardScroll.MaxHeight = height;
        DeviceImage.Height = Math.Clamp(height - 244, 80, 180);
    }

    public void SetState(bool busy, string statusKey)
    {
        StatusText.Text = Localize.Get(statusKey);
        ConnectButton.IsEnabled = !busy;
        Progress.IsActive = busy;
        Progress.Visibility = busy ? Visibility.Visible : Visibility.Collapsed;
        ConnectLabel.Opacity = busy ? 0 : 1;
    }

    public void Render(double opacity) => CardContent.Opacity = opacity;
    private void Connect_Click(object sender, RoutedEventArgs args) => ConnectRequested?.Invoke();
    private void Close_Click(object sender, RoutedEventArgs args) => DismissRequested?.Invoke();
}
