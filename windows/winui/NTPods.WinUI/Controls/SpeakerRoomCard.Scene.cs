using System.Text.Json;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Web.WebView2.Core;
using NTPods.WinUI.Services;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Controls;

public sealed partial class SpeakerRoomCard
{
    private const string SceneHostName = "listening-room.invalid";
    private const string SceneUrl = "https://" + SceneHostName + "/index.html";
    private readonly AccessibilitySettings _accessibility = new();
    private WebView2? _scene;
    private int _sceneDrag = -1;
    private bool _sceneVisible, _sceneReady;
    private bool _windowVisible = true;

    internal void SetWindowVisible(bool visible)
    {
        _windowVisible = visible;
        if (visible && _sceneVisible) LayoutAndDraw();
    }

    internal void SetSceneVisible(bool visible)
    {
        if (_sceneVisible == visible) return;
        _sceneVisible = visible;
        if (!visible) ReleaseScene();
        else if (IsLoaded) OpenScene();
    }

    private async void OpenScene()
    {
        if (_scene != null) return;
        WebView2? view = null;
        try
        {
            view = new WebView2();
            _scene = view;
            AutomationProperties.SetName(view, Localize.Get("Room_Title"));
            SceneContainer.Opacity = 0;
            SceneContainer.IsHitTestVisible = false;
            SceneContainer.Visibility = Visibility.Visible;
            SceneHost.Children.Add(view);
            var profile = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
                "NTPods", "SpatialRoomView");
            var environment = await CoreWebView2Environment.CreateWithOptionsAsync(null, profile, null);
            if (_scene != view) return;
            await view.EnsureCoreWebView2Async(environment);
            if (_scene != view) return;
            var browser = view.CoreWebView2;
            browser.Settings.AreDefaultContextMenusEnabled = false;
            browser.Settings.AreDevToolsEnabled = false;
            browser.Settings.AreHostObjectsAllowed = false;
            browser.Settings.IsZoomControlEnabled = false;
            browser.SetVirtualHostNameToFolderMapping(SceneHostName, Path.Combine(AppContext.BaseDirectory, "Assets", "SpatialRoom"),
                CoreWebView2HostResourceAccessKind.Deny);
            browser.NavigationStarting += (_, args) => args.Cancel = args.Uri != SceneUrl;
            browser.NewWindowRequested += (_, args) => args.Handled = true;
            browser.WebMessageReceived += (_, args) =>
            {
                if (_scene == view && args.Source == SceneUrl) OnSceneMessage(args.WebMessageAsJson);
            };
            browser.NavigationCompleted += (_, args) => { if (_scene == view && !args.IsSuccess) ReleaseScene(); };
            browser.ProcessFailed += (_, _) => { if (_scene == view) DispatcherQueue.TryEnqueue(() => { if (_scene == view) ReleaseScene(); }); };
            browser.Navigate(SceneUrl);
        }
        catch (Exception error)
        {
            // The native map and sliders also work on machines without a usable WebView runtime.
            System.Diagnostics.Debug.WriteLine($"Listening room view initialization: {error.GetType().Name}");
            if (_scene == view) ReleaseScene();
        }
    }

    private void ReleaseScene()
    {
        var view = _scene;
        _scene = null;
        _sceneReady = false;
        _sceneDrag = -1;
        SceneTools.Visibility = SceneContainer.Visibility = Visibility.Collapsed;
        PlanView.Visibility = Visibility.Visible;
        if (view != null)
        {
            SceneHost.Children.Remove(view);
            view.Close();
        }
        LayoutAndDraw();
    }

    private void OnSceneMessage(string json)
    {
        if (json.Length > 256) return;
        try
        {
            using var message = JsonDocument.Parse(json);
            var data = message.RootElement;
            if (data.ValueKind != JsonValueKind.Object || !data.TryGetProperty("type", out var t) || t.ValueKind != JsonValueKind.String) return;
            switch (t.GetString())
            {
                case "ready":
                    _sceneReady = true;
                    PlanView.Visibility = Visibility.Collapsed;
                    SceneTools.Visibility = Visibility.Visible;
                    SceneContainer.Opacity = 1;
                    SceneContainer.IsHitTestVisible = true;
                    Draw();
                    return;
                case "unavailable":
                    var failed = _scene;
                    DispatcherQueue.TryEnqueue(() => { if (_scene == failed) ReleaseScene(); });
                    return;
                case "select":
                case "move":
                    if (!_sceneReady || !data.TryGetProperty("bit", out var id) || id.ValueKind != JsonValueKind.Number ||
                        !id.TryGetInt32(out int bit) || !_nodes.ContainsKey(bit)) return;
                    if (t.GetString() == "select")
                    {
                        bool dragging = data.TryGetProperty("drag", out var drag) && drag.ValueKind == JsonValueKind.True;
                        bool toggle = data.TryGetProperty("toggle", out var ctrl) && ctrl.ValueKind == JsonValueKind.True;
                        SelectSpeaker(bit, toggle, dragging);
                        if (dragging) _sceneDrag = bit;
                        if (data.TryGetProperty("edit", out var edit) && edit.ValueKind == JsonValueKind.True) Placement.IsExpanded = true;
                        return;
                    }
                    if (_sceneDrag != bit || _selection.Active != bit ||
                        !ReadDegrees(data, "az", 180, out double az) || !ReadDegrees(data, "el", 90, out double el)) return;
                    if (_room.DragSelection(_selection.Mask, bit, az, (int)Math.Round(el))) Changed();
                    else UpdateScene();
                    return;
            }
        }
        catch (JsonException) { }
    }

    private static bool ReadDegrees(JsonElement data, string key, int limit, out double angle)
    {
        angle = 0;
        return data.TryGetProperty(key, out var field) && field.ValueKind == JsonValueKind.Number &&
            field.TryGetDouble(out angle) && double.IsFinite(angle) && Math.Abs(angle) <= limit;
    }

    private void UpdateScene()
    {
        var speakers = SpatialRoomLayout.Channels.Where(s => _room.Includes(s.Bit) && s.Bit != 3).Select(s => new
        {
            bit = s.Bit, label = s.Label, name = Localize.Get("Room_" + s.Label),
            az = _room.Bearing(s.Bit) * 180 / Math.PI, el = _room.Get(s.Bit).Elevation,
            active = ((_frame?.Active ?? 0) & (1 << s.Bit)) != 0, level = _frame?.Level(s.Bit) ?? -60
        });
        SendScene(JsonSerializer.Serialize(new
        {
            type = "room", speakers, selectedMask = _selection.Mask, yaw = _frame?.Yaw, pitch = _frame?.Pitch, roll = _frame?.Roll,
            theme = _accessibility.HighContrast ? "contrast" : ActualTheme == ElementTheme.Dark ? "dark" : "light",
            title = Localize.Get("Room_Title"), lang = Loc.Instance.CurrentCulture,
            screen = Localize.Get("Room_Screen"), hint = SceneHint.Text + " " + Localize.Get("Room_CameraKeys")
        }));
    }

    private void SendScene(string message)
    {
        try { _scene?.CoreWebView2?.PostWebMessageAsJson(message); }
        catch (Exception error) when (error is System.Runtime.InteropServices.COMException or InvalidOperationException)
        {
            ReleaseScene();
        }
    }

    private void ResetView_Click(object sender, RoutedEventArgs args) => SendScene("{\"type\":\"resetView\"}");
}
