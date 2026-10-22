# NTPods and LibrePods companion protocol v2

This contract covers app-directed transfer and selected local-preference sync between an
already paired Windows PC and Android phone. Windows keeps its configured Apple
Bluetooth DID; Android keeps its native DID. The companion channel does not
change either identity, pair AirPods, send undocumented routing commands, or
implement Apple's automatic switching. The tested two-way pilot is described
below; stability testing is incomplete.

For manual transfer, the user pauses source media and neither side starts
destination music. Optional incoming-call transfer can pause and later resume
only the exact PC player it interrupted, under the call lease below. The Windows
daemon coordinates both directions; Android owns its client socket in the
existing foreground service. The new preference and call extension has not yet
been validated on hardware.

The PC link is optional for ordinary phone use. When the PC is offline, an
explicit Phone action or AirPods device-card Connect uses local acquisition,
not a companion transaction. It retires older handoff work and durably clears
only the selected pair's release hold. If Android already has that pair's audio
output, LibrePods reconnects AACP controls without reopening the audio profile.
Passive BLE and ACL events do not clear the hold or authorize audio acquisition.

## Transport and trust

- Windows listens on encrypted, authenticated Bluetooth Classic RFCOMM service
  UUID `bbc1e9f2-6f73-4c61-ae15-6e74706f6473`; Android is the client.
- Both hosts must already be bonded. Both applications explicitly allowlist the
  other host's Bluetooth address and verify the actual socket peer, not a
  claimed JSON identity. Refuse unbonded, unencrypted or unauthorized peers.
- An explicitly provisioned, cryptographically random 32-byte secret is unique
  to this host pair. Provision it through the local setup UI, never through an
  unauthenticated RFCOMM connection. Its hex representation has 64 lowercase
  characters. Store it with Windows DPAPI or Android Keystore protection; exclude
  it and its settings from plaintext exports, logs and cloud/device backups.
- Bluetooth link security protects the stream. The handshake below additionally
  authenticates these applications using the shared secret. It does not authorize
  any other host or accessory, and does not add a separate payload encryption layer.
- Allow one authenticated companion session and one active transfer at a time.
  A second socket must not replace an active session or its pending operations.

## Framing and common fields

Each frame is one strict UTF-8 JSON object followed by a single LF byte. The
object is at most 8192 bytes, excluding LF. Bound the buffer before parsing;
reject overflow without waiting for LF. RFCOMM reads may split or combine frames.
Reject BOM, CR, empty frames, malformed UTF-8/JSON, duplicate keys, unknown keys,
unknown message types, incorrect value types and nesting outside these schemas.
No key or value contains a literal line break. Do not accept floating-point `2`
as the version integer. Any framing, authentication or schema violation closes
the socket and retires its pending transaction without reconnecting audio.

Every frame has exactly these common keys plus its table-listed fields:

| Key | Value |
| --- | --- |
| `v` | JSON integer `2` |
| `type` | One of the message names below |
| `tx` | Exactly 16 lowercase hex characters |
| `device` | The selected AirPods' Classic address, uppercase `AA:BB:CC:DD:EE:FF` form |

`device` must match the locally selected, bonded AirPods on both hosts before
authentication and before each operation. It is not the case's BLE address.
Reject all-zero, broadcast and placeholder identities. The MAC used in the vector
below is synthetic. Never accept a wire request to select another accessory.

`0000000000000000` is reserved for handshake, ping, status, preferences,
call_origin, call_state and call_end. A transfer uses a
cryptographically random, nonzero 8-byte `tx`, hex-encoded. Android supplies it
for an Android-originated switch request; Windows allocates it for a local one.
Do not reuse it within a session. Remember accepted and rejected transaction IDs;
after 128 distinct IDs close and authenticate a fresh session before accepting
another. Do not automatically replay a transfer after reconnection.

## Mutual authentication

The only valid initial sequence is `hello`, `challenge`, `auth`, `authenticated`.
Complete it within 10 seconds of socket acceptance using a monotonic deadline.
Each side generates a fresh 32-byte cryptographic random nonce for each socket.
The client validates the server proof before sending its own proof. Each side
decodes the other proof and compares all 32 bytes in constant time.

Construct transcript `T` from these exact ASCII bytes. Substitute the validated
canonical address and lowercase nonce hex. Every line, including the last, ends
with LF. There are no CR or NUL bytes, spaces or JSON serialization in `T`.

```text
NTPODS-COMPANION/2
service=bbc1e9f2-6f73-4c61-ae15-6e74706f6473
client=android
server=pc
device=<device>
client_nonce=<client nonce hex>
server_nonce=<server nonce hex>
client_caps=<client capability hex>
server_caps=<server capability hex>
```

```text
server proof = HMAC-SHA256(raw 32-byte secret, ASCII("server\n") || T)
client proof = HMAC-SHA256(raw 32-byte secret, ASCII("client\n") || T)
sid          = SHA256(T)
```

Proofs and `sid` are lowercase hex of the complete 32-byte result. The transcript
contains nonce *text*, not decoded nonce bytes. The shared secret is decoded
bytes, not its hex text. Distinct proof prefixes bind each role; `T` binds both
nonces, the service, version, capabilities and selected accessory to this session.
`caps` is two lowercase hex characters: bit `01` supports preferences and bit
`02` supports calls. Reject other bits; enable only the intersection. No silent
v1 fallback or alternate automatic behavior is permitted.

| Type | Sender | Additional required keys |
| --- | --- | --- |
| `hello` | Android | `nonce`: client nonce, lowercase hex64; `caps` |
| `challenge` | PC | `nonce`: server nonce, lowercase hex64; `proof`: server proof; `sid`; `caps` |
| `auth` | Android | `proof`: client proof; `sid` |
| `authenticated` | PC | `sid` |

These four frames use zero `tx`. After `hello`, every frame contains exactly the
computed `sid`; reject a different or malformed value. No status or command is
accepted before the final `authenticated` frame. A failed handshake closes the
socket without sending secret values, proofs or detailed authentication errors
to general logs. A reconnect always starts with fresh nonces and a fresh session.

## Authenticated messages

All messages below add the validated `sid` to the common fields. No fields are
optional. Boolean fields are JSON booleans. The values below are case sensitive.

| Type | Sender | Additional required keys |
| --- | --- | --- |
| `ping` | Either | None; zero `tx` |
| `status` | Either | `connected`, `busy`, `can_release`, `can_acquire`: booleans; zero `tx` |
| `switch` | Android | `target`: `"pc"` or `"android"`; nonzero `tx` |
| `prepare` | PC | `target`; `purpose`; `episode`; `call_gen`; `origin_token`; nonzero `tx` |
| `release` | PC | None; active nonzero `tx` |
| `acquire` | PC | None; active nonzero `tx` |
| `result` | See below | `phase`, `ok`, `code`; active or rejected nonzero `tx` |
| `cancel` | Either | `reason`: `"user"`, `"timeout"`, `"lost"`, `"superseded"` or `"failed"`; active nonzero `tx` |

`prepare` uses purpose `manual`, `call_takeover` or `call_return`. Manual uses
zero episode (hex32), call_gen (hex16) and origin_token (hex32). Call contexts
are nonzero and bound as described below. Target is `pc` or `android`.

`connected` means that the selected AirPods' local audio route is connected;
an AACP socket alone does not satisfy it. `busy` includes a call, microphone
capture or another operation that must not be interrupted. `can_release` and
`can_acquire` describe current platform access and local policy. Status is
advisory, not a reservation, a command or proof of the current audio owner.
Refresh the actual local state at prepare and before each destructive operation.
Send status immediately after authentication and in response to ping. Send ping
every 10 seconds; close after 45 seconds without any valid peer frame. Service
work must not block heartbeat processing.

Android reports each requested phase with `result`. Windows reports the final
outcome to Android with `result` phase `complete`, even when acquisition happened
locally. This final message clears the Android reservation. Windows may also
reject an Android `switch` using phase `complete` before sending `prepare`.
Android accepts a successful `complete` only after its prepared local release
or acquisition reached `done`; a failure may reject a request before prepare.
Android answers a PC `cancel` with phase `cancel`. Windows answers an Android
`cancel` with a failed phase `complete`, using `timeout`, `superseded` or `failed`
as appropriate; Windows never sends another `result` phase to Android.

| `phase` | `ok: true` requires `code` | Meaning |
| --- | --- | --- |
| `prepare` | `ready` | Android reserved its current generation and can perform its role |
| `release` | `released` | Android confirmed its local audio release |
| `acquire` | `acquired` | Android confirmed its local destination route |
| `cancel` | `cancelled` | The peer retired the reservation; not a promise to undo an in-flight OS call |
| `complete` | `acquired` | Windows confirmed the destination route and completed this transaction |

For call takeover only, successful acquire and complete use `hfp_ready` instead
of `acquired`; this confirms selected HFP connection/active-device readiness,
not an answered call or SCO audio.

For any phase, `ok: false` requires exactly one failure code: `busy`,
`unavailable`, `invalid_state`, `superseded`, `timeout` or `failed`. Do not use
successful codes with `ok: false` or failure codes with `ok: true`. The phase
must match the command currently outstanding. An unrelated result never advances
a transaction. No arbitrary error text or opaque device payload travels here.

## Transfer state and cancellation

Windows serializes the following sequence for both directions. Each phase has
a 30-second monotonic limit and the whole transfer a 75-second monotonic limit.
Start the total timer when Windows accepts the request, before preflight. Android
starts its reservation timer at `prepare`; it expires after 75 seconds at most,
with a 30-second bound on any phase work. Never extend timers on repeated frames,
status or ping. Peer wall clocks are not used.

1. Check that both applications are enabled for this peer and selected accessory.
   Refresh local state and reserve the Windows connection-intent generation.
   Reject busy or unavailable routes. Keep only one active transaction; reject
   simultaneous switch requests with `complete`, `ok: false`, `code: "busy"`.
   Android remembers its requested target and requires the matching `prepare`
   to keep it. If no prepare arrives within 30 seconds of `switch`, Android
   cancels and retires the ID; a late prepare cannot revive that request.
2. Send Android `prepare` with the destination. Android refreshes its local state,
   validates its source or destination role, and reserves its own intent generation
   without changing audio. Proceed only on its matching `ready` result and while
   the Windows generation and both deadlines are still current.
3. Release the source. For target `pc`, send Android `release` and require
   `released`. For target `android`, release Windows locally and await confirmation.
   Suppress only the transaction's existing automatic reconnect intent. Confirm
   that source playback and microphone routes are closed; a queued API call or
   successful write alone is insufficient. Keep the companion socket alive.
4. Acquire the destination only after source release is confirmed. For target
   `android`, send `acquire` and require `acquired`. For target `pc`, acquire
   Windows locally and await the current route confirmation. An API return,
   cached endpoint or control connection alone is insufficient. Recheck generation
   and deadlines before acting and when accepting completion.
5. Send `result` phase `complete` with the outcome. Clear only this transaction's
   reservation. Manual transfer does not start, resume or pause media.

`prepare`, `release` and `acquire` are accepted only from Windows, in their
permitted order for the active `tx`, `sid`, device and role. Neither endpoint
retries a command within a transaction. A repeated phase must never execute again.
Reject a reused ID or out-of-order frame without touching audio. An endpoint may
close the session on such a protocol violation; it must retire pending work.

Any explicit local Connect, Disconnect, pair selection, unrelated microphone/call start,
companion disable, unpairing or authorization change invalidates the local intent
generation. Cancel pending transfer work and notify the peer if possible. Check
the captured generation immediately before each OS operation and on completion.
Delayed callbacks cannot initiate another operation, change a newer user's
connection intent, clear a newer request or report an obsolete operation as success.

Windows serializes local actions and transfer dispatch at a reservation gate.
A local action invalidates the transfer when it is processed at that gate; it
cannot retract an OS operation or send that already reserved the gate. The
30-second HFP cleanup wait runs on a separate worker and reports its receipt.
Socket writes wait at most one second. Read-only platform queries and local
driver or storage I/O still have OS-dependent latency, so this is not a hard
real-time guarantee that an input callback runs immediately.

For manual transfers and invalidated call leases, on `cancel`, phase timeout,
control loss or RFCOMM loss, stop issuing operations
and retire this transaction. Do not automatically reconnect the source, disconnect
the destination, resume media or retry takeover. A platform call already in flight
may still finish: cancellation does not make it disappear. Report uncertainty
locally and require a new explicit request rather than undoing newer user actions.
Only rollback that is proven to belong to the still-current local generation may
restore temporary local bookkeeping; it must not reconnect or switch audio.

## Preferences

Preference frames require capability bit `01`; call frames require bit `02`.

`pref` has exactly the common authenticated fields plus `key,rev,origin` and the
payload selected by the following fixed key. `rev` is a nonzero unsigned 64-bit
integer encoded as 16 lowercase hex characters; `origin` is `pc` or `android`.
There are no arbitrary setting names, arbitrary bytes or raw datastore copying.

| key | exact payload fields and values |
| --- | --- |
| host_auto_pause | `value`: boolean; host pause/resume policy, independent of accessory ear-detection configuration |
| theme | `value`: `system`, `light`, `dark` |
| conversation_policy | `mode`: `abs_cap` or `relative`; `first,second`: canonical decimal percent strings; `pause_media`: boolean |
| connection_popup_enabled | `value`: boolean; master gate for automatic accessory popup feedback, not explicit Show batteries |
| cache_disconnected_battery | `value`: boolean; retain disconnected component battery readings, not model metadata or history replication |
| call_takeover_enabled | `value`: boolean, initially false |
| call_return_enabled | `value`: boolean, initially true; inactive until takeover is enabled |
| desired_microphone_side | `value`: `unset`, `auto`, `right`, `left` |
| desired_ear_detection | `value`: `unset`, `on`, `off` |
| desired_hold_actions | `left,right`: each `unset`, `listening`, `assistant`; either both unset or both set |
| desired_noise_modes_cycle | `value`: `unset` or two lowercase hex digits, a low-four-bit mask containing at least two modes |
| desired_swipe_duration | `value`: `unset`, `0`, `1`, `2` |
| desired_case_sounds | `value`: `unset`, `on`, `off` |
| desired_case_volume | `value`: `unset` or canonical integer string from `0` through `100` |
| desired_allow_off | `value`: `unset`, `on`, `off`; supported silent Allow Off models only |

Canonical decimal percentages match
`(?:0|[1-9][0-9]?|100)(?:\.[0-9]{0,8}[1-9])?`, are at most 13 ASCII bytes,
have numeric value in 0..100, and satisfy second <= first. No exponent, sign,
leading zero or trailing fractional zero is permitted. Compare as decimal,
not binary floating point. Existing Windows stage-two values seed exact
first*6/10 decimal text; Android finite saved floats use shortest decimal text
without exponent or trailing zero. An unrepresentable legacy value remains
local with an explicit migration failure; it is not silently rounded.

`pref_ack` has exactly common authenticated fields plus `key,rev,origin,code`.
Codes are `applied,unchanged,unsupported,failed,stale`, acknowledging the
received stamp. Unknown keys or malformed values are schema failures. A known
valid key unsupported by this host/model receives `unsupported`, preserving
the local setting. A failed durable write receives `failed` and does not apply.

Each field has a durable Lamport stamp. Compare unsigned revision first, then
`pc` greater than `android` for equal revisions. An actual local edit chooses
max observed revision for that field plus one and the local origin. Remote
application durably preserves the received stamp and never creates a local
edit or echoes it. Equal stamp/equal payload is idempotent; equal stamp with a
different payload is a collision and closes the session. Lower incoming stamps
receive `stale`; the receiver may offer its winner once for that received stamp.
Counter overflow refuses further edits for that field rather than wrapping.
Value and stamp must be committed together before applying or acknowledging.

After authentication, offer each supported current record once. Offer actual
local edits once; acknowledgements never trigger offers. Reconnection reconciles
per field rather than replaying commands. Theme is a host-global field with one
global stamp; the other records are scoped to the paired accessory. Existing
local values are migrated once, without being rewritten as a new edit on each
startup. A UI event that displays a remote value must not send it back.

Desired accessory choices are saved, explicitly unconfirmed intent. They never
change confirmed readback, issue an AACP write on receipt/reconnect, or bypass
model/capability/Allow Off validation. Fresh accessory readback remains
authoritative for the displayed effective device setting. Android delegated
host hold actions are not equivalent to hardware hold configuration and must
not be mapped without a deliberate adapter.

Disconnected battery presentation hides cached readings while retaining their
original observations and age for re-enabling. It does not erase samples or
model metadata, and live nearby readings remain visible.

Local automation consent and OS permissions are not preferences. Receiving true
call preferences cannot grant either. Effective call automation requires the
shared desired policy, explicit local consent and current platform permissions.
Revoking consent or changing call policy invalidates existing call leases.

## Calls and ownership

PC remains the coordinator. All existing manual transfer behavior remains:
no automatic destination music, rollback or ownership claim from HeldAway,
control connectivity or a cached preference.

Additional exact authenticated frame keysets are:

| type and sender | extra fields beyond common authenticated fields |
| --- | --- |
| call_origin, PC | `origin_token`: 32 lowercase hex; `ready`: boolean |
| call_begin, Android | `episode`: nonzero 32 lowercase hex; `call_gen`: nonzero 16 lowercase hex; `origin_token`: nonzero 32 lowercase hex; nonzero tx |
| call_state, Android | `episode,call_gen`; `state`: `ringing,active,idle,unknown`; `hfp_ready`: boolean; `sco`: `idle,connected,unknown`; `all_idle`: boolean |
| call_end, Android | `episode,call_gen,origin_token`; `reason`: `ended` or `invalidated` |

Every v2 `prepare` has `target,purpose,episode,call_gen,origin_token` in addition
to common authenticated fields. `purpose` is `manual,call_takeover,call_return`.
Manual uses all-zero episode, generation and origin token. Call takeover is
target android; call return is target pc. Other phase frames retain their
existing exact keysets and use context bound by prepare. A call takeover
acquire/complete uses success code `hfp_ready`; it means selected HFP connected
and active, not answered call or observed SCO. Other phase/code rules remain.

The PC advertises a random origin token only while the selected paired device
has an actual authorized ready PC audio route, no transfer/busy owner, and
matching explicit request, audio intent, native session and local policy
generations. `ready:false` requires a zero token. Any relevant newer choice or
route-readiness loss revokes the token. Android snapshots an eligible token
already received before a new incoming RINGING episode; a token first received
after ringing cannot authorize that episode. PC rechecks every bound condition
before accepting call_begin. One incoming episode gets one takeover attempt.

The episode is random and local; call_gen fences its local reservation. Neither
contains a number, contact or subscription identifier. Outgoing OFFHOOK without
preceding observed RINGING is ineligible. All active subscriptions must be
observed, or the implementation must refuse automation when coverage is unknown.
Unknown telephony, lost permission, another call, user override, microphone or
capture activity, selection/policy changes, restart and session loss retire the
lease. Transport reconnect never restores a call or resume lease.

Android prepares a call takeover only for its exact current incoming reservation.
That reservation exempts only its own verified telephony/audio-call mode from
ordinary Android acquire vetoes. It does not bypass unrelated busy owners,
microphone/capture guards, other calls, selected-SCO ownership, binder liveness
or generation checks. PC generic busy checks remain. PC pauses a proven selected
player before source release and retains a local causal pause ticket; player,
track and media identity never travel over this link.

Ringing does not require SCO. `hfp_ready` requires selected HFP connection and
active-device proof from a live service; `state:active` requires local OFFHOOK;
`sco:connected` requires selected SCO observation. Do not infer one from another.
call_state is sent on change and every ten seconds while a call lease exists;
no fresh call state for thirty seconds retires automatic return eligibility.
During a matching, current, bounded call-takeover transaction, Android may send
fresh telephony state with `hfp_ready:false,sco:unknown` while the native worker
owns the route query. This keeps call-state liveness without claiming HFP or SCO
readiness. Windows accepts that unknown SCO only for the matching active phase;
it never establishes quiet proof for return or resume. Call end still requires
a fresh selected-SCO-idle receipt after worker drain.

A provisional PC origin/pause lease exists before release. Confirmed source
release marks that fact independently of Android HFP completion. The long call
lease is separate from the 75-second transfer and retains exact episode, session,
pair, policy, manual-intent and owner generations. No failed acquire is replayed.

Natural end and invalidation differ. Before `call_end:ended`, Android retires
and drains pending call-acquire mutations, verifies all monitored calls idle,
MODE_NORMAL and selected raw SCO idle with live service. Failure to prove this
uses invalidated and forfeits automatic return. Ended may preserve the same
provisional lease even if HFP acquire had not completed. PC retires the old
transfer, then may start a NEW call_return transaction. Its fresh Android
prepare/release must prove pending call workers quiescent and every ordinary
native/profile/control owner released before PC acquire. Same-episode end is
not permission for blind rollback. If PC had not released, only fresh same-origin
PC readiness plus phone-quiet proof and an unchanged owned pause ticket permit
local resume; otherwise it remains paused with an error.

After verified return, the PC may consume only the exact player pause ticket it
caused, with all fences still current and no intervening player choice. It never
starts a previously paused player, resumes another player, or retries a consumed
ticket. Unknown state or loss of proof means no automatic resume. User override
or session loss always revokes both return and resume rather than scheduling
cleanup-driven return.

## Local-only inventory

These settings remain host-specific because no exact counterpart exists:
Windows codec/bitrate/link/game/buffer/native-clock experiments, mic policy and
delay, hotkey, startup and OS permission; Android BLE scanning/delay, delegated
head/hold gestures and calibration, heart-rate alerts, design system/material
color/fonts/accessibility, refresh/debug/navigation, detailed Island geometry,
widget identity/layout and unused legacy takeover toggles. App language remains
local until both platforms have an explicit common-locale adapter. Credentials,
app identity, permissions, pairing/recovery journals, battery samples/timestamps
and diagnostic history never sync. Existing unused takeover flags never seed the
new call policies. Takeover defaults off; return defaults on but stays inactive
until the user enables takeover locally in both apps.

## Implementation evidence and pilot limits

Software checks cover authentication vectors, strict framing and schemas,
transaction ordering, stale completions, and Rust/C# status and command fixtures
with both destinations and full-width request IDs. V2 preference synchronization
and call automation have not been verified on hardware.

The pre-v2 manual pilot completed transfers in both directions between Windows
and a Pixel running Android 17 with AirPods Pro 3. Both destinations returned
current acquisition receipts, and the user confirmed clean audio. Windows kept
its existing vendor identity and Android used its native identity. The evidence
covers this hardware and cooperative flow; Apple automatic switching and other
models remain unverified.

Android 78 confirmed the exact selected output 7.4 seconds after its profile
connected, completing PC-to-phone transfer in about 17 seconds. Its output wait
is bounded to 10 seconds within the 24-second local budget and 30-second phase
limit; exact-output and cancellation checks remain required. The four-second wait
had reported a false failure despite eventual audio. A phone-to-PC transfer
completed in 6.3 seconds. The diagnostic Windows UI roundtrip completed in
15.7 seconds to the phone and 7.1 seconds to the PC, with matching terminal results
and both buttons enabled afterward.

Windows restores HFP services after a native audio reset before reconnecting
control. An ordinary Connect baseline stayed ready for 140 seconds with clean
playback. A captured handoff return completed HFP setup before AAP and kept its
ACL and native channels open for 42.7 seconds after readiness; HFP itself closed
and reopened. The app stayed ready through about 90 seconds idle, and the user
confirmed clean playback on resume without reconnect sounds. A 120-second
observation with tracing disabled also kept the PC route ready and connected
with user-confirmed clean audio.

Observed limits include rejected peer-initiated configuration recovered by an
outgoing retry, peer ACL loss with repeated idle recovery, and an unreproduced
Windows UI Switching stall after transfer completion. The 80-second missing-result
UI recovery has only software coverage. Repeated transfers, long playback and
idle/reconnect stability still need testing.

The pilot used the apps' transfer and cancellation controls. Android Connect or
Disconnect in OS Settings does not invoke the app's explicit-intent callback,
and profile events cannot reliably identify that user intent. An OS Settings
action during transfer therefore lacks the same immediate invalidation guarantee
and needs separate hardware validation.

Windows confirms release from the matching transaction's control, native playback
and audio profile cleanup receipts. Every receipt must succeed; the driver handle
must be released, the microphone quiet, audio cleanup finished, and native playback
inactive. Missing, failed or stale receipts block destination acquisition. Windows
rechecks the current intent generation and deadlines before sending Android
`acquire`, and keeps its automatic reconnect intent held away.

Whole Classic ACL connectivity is advisory: an idle AVRCP or other stack link may
remain after all audio owners are released. ACL disconnection alone does not prove
release. Neither side automatically reconnects the source on timeout or failure.
An independent OS reconnect at the destination does not prove the companion sent
`acquire` or completed the transaction.

## Interoperability vector

This vector is public test data, never a deployment secret. Device is
`AA:BB:CC:DD:EE:FF`. All hex strings below are continuous, without whitespace.

```text
secret       = 000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f
client_nonce = 202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f
server_nonce = 404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f
client_caps  = 03
server_caps  = 03
T length     = 300 bytes
sid          = 628870c5318e3d519391f2a559fe37c94a7fec6cef8b028ea367504a86a582c8
server proof = 5b4030aa163ea0085a1618f282393d0ed62817e38012461823cab87699b621ff
client proof = 2a6ab41f49a296aae9cf74fb70c56c9998912c7e0c022b24ce2209f1c347fca4
```

The full v2 schema/auth fixtures are identical in
`ntpods/windows/daemon/tests/fixtures/companion-v2.json` and
`librepods-rewrite/android/app/src/test/resources/companion-v2.json`.

Required software checks cover preference conflicts, durable-write failures,
no echo, reconnect reconciliation, stale call origin, call end before and after
source release, pending-HFP cleanup, newer intent and exact-player resume;
split/coalesced reads and bounds; invalid UTF-8,
duplicate/extra keys and wrong types; wrong key/proof/device/session and role
reflection; pre-auth commands; duplicate or reordered phases; simultaneous
requests; timeout and socket loss after release; cancellation and local user
intent during each phase; stale OS completions; and identical vectors on both
platforms. Hardware checks must separately confirm each source's release, actual
destination audio, reverse transfer, busy-call refusal and ordinary reconnect
after a failed transfer. A passing software test does not establish audio handoff.
