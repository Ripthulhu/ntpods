namespace NTPods.WinUI.Controls;

internal sealed class VolumeEditState
{
    private byte _reported;
    public byte? Requested { get; private set; }
    public bool Interacting { get; private set; }
    public byte Displayed => Requested ?? _reported;

    public void Update(byte reported, bool connected)
    {
        _reported = reported;
        if (!connected) Cancel();
        else if (!Interacting && Requested == reported) Requested = null;
    }

    public void BeginInteraction() => Interacting = true;
    public void Request(byte value) => Requested = value;

    public void EndInteraction()
    {
        Interacting = false;
        if (Requested == _reported) Requested = null;
    }

    public void Cancel()
    {
        Interacting = false;
        Requested = null;
    }
}
