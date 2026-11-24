# NTPods for USB AirPods dongles

NTPods is a Windows companion for the [AirPods Dongle](https://github.com/Ripthulhu/airpods-dongle).
Plug in the dongle and pair the AirPods from the **AirPods Dongle** card or
with the dongle's button. If several dongles are attached, select one in the
card. Choose the dongle headphone endpoint in Windows for playback.

Once the AirPods connect, supported battery, listening-mode and device
settings appear in the main settings view. Controls depend on the AirPods
model and dongle firmware. Use the dongle card for pairing a Bluetooth
audio source and inspecting dongle diagnostics.

AirPods Pro 3 settings include spatial audio, calibration and an ear profile
comparison. The ZIP includes the Steam Audio renderer. Select Steam Streaming
Speakers in the game or player to feed it a surround mix.

Download the [Windows x64 dongle preview](https://github.com/Ripthulhu/ntpods/releases/download/dongle-nightly/NTPods-Dongle-Windows-x64.zip).
Extract the ZIP and open `ntpods-winui.exe`. Keep the companion daemon and WinUI runtime in
the extracted directory. Windows provides the USB audio and HID drivers.

[Build the preview](docs/BUILD-WINDOWS.md) · [Dongle setup and controls](docs/PICO-DONGLE.md)
· [Direct Bluetooth development on main](https://github.com/Ripthulhu/ntpods/tree/main)
