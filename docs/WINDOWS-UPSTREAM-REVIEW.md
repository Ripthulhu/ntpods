# LibrePods source review for Windows

Pinned source comparisons from October 2026. These identify reusable protocol
work and platform limits; hardware coverage is in
[model support](WINDOWS-MODEL-SUPPORT.md).

## Pinned sources

| Source | Revision | Transferable work or limit |
| --- | --- | --- |
| [Upstream Android rewrite](https://github.com/librepods-org/librepods/tree/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c) | `34076c85` | Settings, capability and sensor packet definitions; Conversation Awareness host behavior. |
| [Upstream main](https://github.com/librepods-org/librepods/tree/efccc4499b2c1fd2ac243e975ae1f42d8e828ffd) | `efccc449` | Linux Qt and Rust implementations; Linux audio/session APIs need Windows equivalents. |
| [hexarch](https://github.com/hexarch/librepods/tree/f3b1db25136af7e884e7f41580d5644338449d17) | `f3b1db25` | Older tree from December 2025; no newer Windows transport found. |
| [beidaoc](https://github.com/beidaoc/librepods/tree/2aad9c3c5c7dd20e97cc9c1673b19b5373fb9bdf) | `2aad9c3c` | Xiaomi hooks, Android spatializer, Keep heart-rate bridge and routing work. OS hooks are not Windows implementations. |
| [Tblob18 Windows fork](https://github.com/Tblob18/librepods-windows/tree/a439cadb517f6973b5d57dd2e0d3fbfdde16fe97) | `a439cadb` | Qt Windows port; `windows/main.cpp` still opens `QBluetoothSocket(L2capProtocol)`. No replacement for the NTPods Windows AACP driver found. |
| [Chrisbattarbee](https://github.com/Chrisbattarbee/librepods/tree/9b34fdcad7a9d1a345c7f39595850116b777edb4) | `9b34fdca` | Linux Rust stem-event dispatch and rearm fix. |
| [yicareful Chinese fork](https://github.com/yicareful/librepods_Chinese/tree/9c556558e2531a427b2af4e2d470a770fa5c8025) | `9c556558` | Android fork; no Windows transport or audio implementation to import. |
| [arctumn Windows native](https://github.com/arctumn/librepods/tree/eb002807795f9abebffddf296b0e8d9a8d201111) | `eb002807` | Windows predecessor to NTPods. |
| [arctumn heart-rate MTU branch](https://github.com/arctumn/librepods/tree/eda9eae99233c98b61577ec1e9b7cbb23958417b) | `eda9eae9` | Inbound MTU 1691, HR packet parsing and history. NTPods already carries this work. |
| [thibaup Find My](https://github.com/thibaup/librepods/tree/3f4f9391ebb4fd1cef645e081052854a967ff969) | `3f4f9391` | Verified BLE RSSI finder plus Apple-account/cloud discovery. The latter needs a separate authentication and security implementation. |
| [Wtrwx](https://github.com/Wtrwx/librepods/tree/e21b388b7bd775cb39ec9c5f4b8772c9d0afc1af) | `e21b388b` | Xiaomi Bluetooth battery metadata and Android/Mac routing changes; platform hooks do not transfer directly. |

## Protocol and platform findings

1. **Conversation Awareness volume preference and end events.** The rewrite uses
   saved host volume levels in
   [LibrePodsService.kt, lines 813–835](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/services/LibrePodsService.kt#L813)
   and restores on statuses 6, 7, 8 and 9. NTPods handles 4/6/7/8/9 and saves a
   0–100% maximum under **Volume while speaking**.
   Its default remains 25%; the second speech stage uses 60% of that maximum.
   Quiet playback stays quiet, and manual volume changes cancel automatic
   restoration.

2. **Device capability reports.** The rewrite parses message opcode `0x02` using
   the fixed widths in
   [Capability.kt](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/bluetooth/aacp/types/Capability.kt).
   Windows can add bounded parsing and a device-details view without changing
   the handshake. Unknown capability IDs have no encoded width, so a parser must
   reject an undecodable report instead of guessing where the next entry starts.
   A capability report establishes advertised support; it does not establish a
   setting's current value or prove that a write succeeded. NTPods does not yet
   parse this report.

3. **Workspace-use hearing protection setting.** The rewrite's
   [HearingProtectionScreen.kt, lines 103–117](https://github.com/librepods-org/librepods/blob/34076c85f0fc2eaf32ad1d5cad1bb7315df9c89c/android/app/src/main/kotlin/me/kavishdevar/librepods/presentation/screens/apple/HearingProtectionScreen.kt#L103)
   writes AACP control `0x37` as 1/2. It does not use ATT for this switch. A future
   Windows control should require a valid current report, keep requested and
   confirmed values separate, and pass change/readback/restore tests. No
   attenuation or protective-performance claim follows from that stored bit.

4. **Optional custom stem actions.**
   [Chrisbattarbee's stem fix](https://github.com/Chrisbattarbee/librepods/commit/bf09c6795ea9e912da08bd0f82a429e6b1243419)
   rearms raw stem notifications through control `0x39` after each event and
   releases the state lock before sending. Its `linux-rust/src/devices/airpods.rs`
   dispatches a single press to a media controller. A Windows version could map
   explicit custom actions to the selected SMTC session, but must first verify
   which raw-gesture flags suppress ordinary Bluetooth media actions. Enabling
   both paths could immediately pause and resume the same track. The pinned
   rewrite's raw-gesture settings screen is commented out; enum and packet
   definitions alone are not complete custom-action support.

5. **Find nearby using verified RSSI.** thibaup's
   [NearbyAirPodsFinder.kt](https://github.com/thibaup/librepods/blob/3f4f9391ebb4fd1cef645e081052854a967ff969/android/app/src/main/java/me/kavishdevar/librepods/finder/NearbyAirPodsFinder.kt)
   consumes identity-verified BLE scans only while explicitly active. A Windows
   port could show signal getting stronger or weaker using the existing BLE
   watcher. NTPods permits scanning during playback only when enabled and pauses it
   during high-quality capture. A finder must respect that policy and distinguish
   signal loss from distance. Cloud Find My, case ringing and Ultra Wideband
   navigation require separate implementations.

## Existing coverage and unresolved gaps

The pinned rewrite heart-rate fix treats payload byte 1 as unsigned. NTPods' Rust
decoder already reads `u8`, and the Windows MTU and confidence filtering from
arctumn are already present. Removing confidence checks to match the simpler
rewrite parser would discard existing validation. HR remains experimental until
its complete stream lifecycle is checked on the target hardware.

Both applications subscribe to all notifications with `FF FF FF FF`. The rewrite
also sends country code and requests magic keys, and repeats initialization after
three seconds. It has no implemented per-control read command that would recover
all currently unreported settings. Repeating a handshake is not an appropriate
substitute during playback or microphone capture.

The rewrite's main feature visibility uses model-specific `BaseCapability` sets.
An absent current setting report therefore means unknown, not necessarily
unsupported. Windows should keep that distinction when presenting microphone
side, ear detection, hold actions, mode cycle, swipe duration, Allow Off and case
settings.

The rewrite enables Allow Off for supported Pro 3 models without waiting for
`0x34`, and retains its requested value per device. On the Windows test pair,
`0x34=1` produced no report but listening mode Off returned a confirmed `0x0D=1`.
NTPods keeps a separate saved request for this silent setting; it does not
insert the request into confirmed controls or replay it on reconnect.

Hearing assistance and custom transparency use ATT. The rewrite's
`bluetooth/att/ATTManager.kt` still has TODOs in characteristic read/write methods;
its loud-sound-reduction UI also requires the Android vendor-ID hook. NTPods has
an experimental ATT path, but those screens do not prove Windows compatibility.
Head gestures need sensor processing and a real Windows call or application
action. Android camera hooks, Keep integration, Xiaomi system metadata and
Android spatializer code each depend on services Windows does not provide.

Cloud Find My requires Apple-account sessions and separate protocol work. No
reviewed fork supplies a Windows firmware updater or a complete Windows spatial
audio renderer. NTPods' native AAC/AAC-ELD playback is documented separately in the
[audio reference](WINDOWS-AIRPODS-AUDIO-PROTOCOL.md).
