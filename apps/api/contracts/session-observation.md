# Session presence and observation

## Lifecycle and presence

`GET /api/v1/connections/sessions` accepts `view=active|history`, search,
filters, sort, limit and offset. The default view is `active`. The server applies
the view and search before counting and paginating. A terminal status or a
persisted `ended_at` places a session in History. Duration is measured from
`started_at` to `ended_at` for a finished session, not to the current time.

Persistent lifecycle and runtime presence answer different questions:

- `pending` and `active` are nonterminal database states.
- `disconnected`, `terminated` and `failed` are terminal database states. An
  explicit disconnect closes the remote runtime; it is not a browser detach.
- `presence.state` reports `connecting`, `live`, `detached`, `degraded`, `ended` or `unknown`
  from the proxy that owns the runtime. A browser may detach while the remote
  session remains alive within its reconnect grace period.
- No output or input for a long time does not end a session. Traffic rates and
  `last_activity` are not a heartbeat and do not establish operator presence.
- `presence.last_operator_activity_at` records accepted controller input, not
  remote output, observer activity, acknowledgements or transport heartbeats.
- An unavailable proxy or missing runtime evidence yields `unknown`. The API
  does not invent a green live state, an observer count, or an end timestamp.

`presence` also carries the runtime generation, the observation timestamp,
operator attachment, observer count and any known reconnect deadline or end
time. The session health endpoint exposes the same presence contract. Counts
describe attached observer legs, not recordings or historical viewers.

Resume attaches to the same alive runtime. Once that runtime ends, a new login
requires a new connection session. Runtime generations prevent an old observer
credential from following a replacement connection.
A session without a trusted runtime-generation pin remains unconfirmed; it is
not silently rebound to another runtime or assigned an invented end time.

## Authorization

Observation requires both `OBSERVE` on the concrete `CONNECTION_SESSION` and
`OBSERVE` on that session's concrete `CONNECTION_TARGET`. Policy attributes come
from the stored session and target. `READ`, `CONNECT` and `EXECUTE` do not grant
observation. Adding the action/resource pair does not alter existing grants.

The session response exposes an `observe` capability with `allowed`, `reason`,
`websocket_url` and `transport`. A capability is presentation information, not
an authorization token. The observation websocket authorizes again at attach.

`GET /api/v1/ws/observe/{session_id}` uses the API's authenticated identity.
The API revalidates the credential, current policy, session and generation
every five seconds, with a three-second check deadline. Forwarding pauses
during these checks and stops on denial, expiry, failure or generation change.
No target credential is returned to an observer. The API-to-proxy credential is
bound to observer mode, actor, connection and runtime generation; operator
endpoints reject it. The proxy also ends the observer leg at credential expiry.

## Transport boundaries

SSH observations use the existing PTY. The proxy maintains parsed terminal
state, including primary and alternate screens, cursor and dimensions. A quiet
join receives a complete snapshot. A server control frame
`__piping_ws__:{"type":"observer_resize","cols":80,"rows":24}` sets the
observer's local renderer size before the following binary snapshot. It is not
a request to resize the remote PTY. SSH observer application input is rejected.

RDP and browser VNC use the same Guacamole desktop transport. The proxy retains
the original guacd connection ID and joins that ID for each observer. It never
selects a remote protocol or supplies target login details to start an observer.
Native shared-user synchronization supplies the current desktop on a quiet
join. Browser VNC therefore does not consume a copied midstream RFB handshake.
The bundled Guacamole RDP plugin gives each writable joined operator its own
native object for the existing shared drive. Listing, upload and download
streams stay on that operator's native user connection. Read-only joins receive
no drive object; filesystem traffic is never relayed from the retained owner to
an observer. An externally supplied guacd must include the same native patch.

The desktop observer handler accepts only acknowledgements needed for its own
display leg. Keyboard, mouse, display resize, clipboard, files, target control,
and post-handshake connection negotiation are forbidden. Clipboard and file
streams are not exposed to the observer. The read-only renderer is defense in
depth; the server enforces this boundary for custom or malicious clients.
The Guacamole tunnel's bounded internal ping is echoed by the API itself. It
never reaches the remote desktop and is not operator activity.

Leaving observation closes only that observer leg. It does not terminate the
operator, change the owner, extend operator reconnect grace or acquire control.
Ending the remote session closes its observers. The operator sees the
proxy-reported observer count; unavailable presence is shown as unknown.
The proxy permits at most 32 simultaneous observer legs per session.

## Audit and privacy

Observe start, stop and deny events identify the actor, session, time and a
bounded reason code. Audit metadata contains no terminal output, screen data,
keystrokes or credentials. Start auditing must succeed before display data is
forwarded. Audit storage failures are surfaced as failures, not successful
audit receipts. Terminal and desktop state used for live observation remains
in the runtime; this feature does not add persistent screen recording.

Session, transport and observation producers attach the shared typed audit
context. Session and transport states remain distinct from the operation
outcome; automatic transport endings do not name the session owner as their
initiator. Request and session correlations are UUIDs when established.
The observe-start audit gate runs before the proxy handshake, so it records an
allowed authorization decision with an unknown operation outcome, not proof of
a connected display. Missing event-time display labels remain absent.
