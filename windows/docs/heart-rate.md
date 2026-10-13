# Heart rate on Windows

NTPods requests one heart-rate reading per second on AirPods Pro 3. The Heart
Rate section shows live readings and saved sessions. Monitoring uses extra battery;
model support is defined in [`device-models.json`](../device-models.json).

## L2CAP requirements

The AAP driver requests an inbound MTU of 1691 bytes on PSM `0x1001`. Heart-rate
service descriptors observed on Pro 3 are about 920 bytes, larger than the default
672-byte L2CAP MTU. Acknowledgements without readings can therefore indicate that
the channel never received the necessary descriptors.

[`L2cap.c`](../drivers/aap/L2cap.c) sets:

```c
brb->ConfigIn.Flags         = CFG_MTU;
brb->ConfigIn.Mtu.Min       = 672;
brb->ConfigIn.Mtu.Preferred = 1691;
brb->ConfigIn.Mtu.Max       = 1691;
```

`ConfigIn` controls incoming data; setting only `ConfigOut` does not raise the
inbound MTU. The driver also requests an authenticated, encrypted link. The
heart-rate implementation uses that combination; the independent requirement
for each security flag has not been established.

## Starting the stream

[`hr_retry_campaign`](../daemon/src/main.rs) initializes AAP services 0 and 4,
sends `HRM_STATE` (`0x30`) with value `0x01`, waits 120 ms, then starts service 84
(`HEARTRATE_COMMAND`) and legacy service 19 at a one-second interval. Activation
IDs remain firmware-specific even when descriptor discovery supplies a different
report ID.

The daemon waits up to 12 seconds for a decoded reading and makes at most four
attempts, with bounded backoff. It does not reconnect the shared Bluetooth channel
for a failed optional sensor. Turning monitoring off cancels the request; turning
it back on starts another attempt.

[`aap.rs`](../daemon/src/aap.rs) defines the packets. The start frame uses
SensorDataWX with a service setting of `2` and configuration bytes
`01 40 42 0F 00`: enable plus a little-endian 1,000,000-microsecond interval.
Newer firmware can acknowledge service 84 through service 20 (`HEARTRATEv2`).
The active host must own the AirPods connection for sensor requests to work.

## Reading the reports

[`hr.rs`](../daemon/src/hr.rs) reassembles bounded RTBuddy frames and accepts live
log types 1 and 3. It uses the discovered report service when available, otherwise
legacy services 19, 20 and 84. The heart-rate payload is 18 bytes:

| Offset | Meaning |
| --- | --- |
| 1 | BPM |
| 2 | Confidence |
| 3 | Observed sample counter; not used to calculate BPM |
| 15–17 | Status trailer |

A valid sample has BPM 30–220, confidence at least `0x80`, and one of these
trailers: `10 00 00`, `10 00 80`, `20 00 00`, `20 80 00`, `20 02 80`, or `20 82 80`.
Warm-up readings can have a plausible BPM but low confidence and are discarded.
Unknown trailers are rejected rather than treated as valid measurements.

On the captured Pro 3 firmware, RTBuddy service 14 is the activity classifier.
Older firmware can use it for 58-byte movement reports, so the decoder checks
the payload and rejects short activity reports. Heart-rate setup does not stop
motion services. See the [protocol reference](../../docs/AIRPODS-PROTOCOL.md)
for descriptor discovery.

## Saved readings

[`hrdb.rs`](../daemon/src/hrdb.rs) writes accepted readings on a separate thread
so storage does not block Bluetooth reception. Data stays in
`%LOCALAPPDATA%\NTPods\heart-rate.sqlite3`:

- `sessions`: device identity, start and end time.
- `samples`: session, Unix timestamp in milliseconds, BPM and confidence.

The app reads this database for history. If it cannot be opened, live monitoring
continues without saving readings. Keep the database out of bug reports unless
its contents have been reviewed and sharing is intended.

The enable sequence and legacy report layouts follow
[LibrePods PR #702](https://github.com/librepods-org/librepods/pull/702).
Hardware coverage is recorded in
[WINDOWS-VALIDATION.md](../../docs/WINDOWS-VALIDATION.md).
