using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Services;

internal sealed class DeviceViewIdentity
{
    private string _deviceId = "";
    private string _model = "";

    public bool Update(Snapshot snapshot)
    {
        // Connection and battery reports can omit identity while the same device sleeps.
        if (string.IsNullOrEmpty(snapshot.DeviceId)) return false;
        bool hadIdentity = _deviceId.Length > 0;
        bool newIdentity = _deviceId != snapshot.DeviceId;
        bool changed = hadIdentity && newIdentity;
        if (newIdentity) _model = "";
        _deviceId = snapshot.DeviceId;
        string model = DeviceArt.ResolveModel(snapshot.Model, "", "");
        if (model.Length > 0)
        {
            var profile = DeviceModels.Find(model);
            if (profile.Known) model = profile.Models[0];
            changed |= hadIdentity && !newIdentity && model != _model;
            _model = model;
        }
        return changed;
    }
}
