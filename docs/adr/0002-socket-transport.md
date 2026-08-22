# 2. Talk to herdr over its socket, not its CLI

Status: accepted

## Context

herdr documents the whole CLI as the plugin API, and also injects `HERDR_SOCKET_PATH` into
every plugin command. Either is a legitimate way in. Shelling out to the CLI is the more
obvious choice: it is what the documentation shows, and it insulates a plugin from the wire
protocol.

## Decision

Use the socket.

The deciding factor is `pane.focus`. Jumping to a chosen pane is the entire point of the
panes view, and the CLI cannot do it:

- `herdr pane focus` is directional — `--direction left|right|up|down`. There is no
  `pane focus <pane_id>`.
- `herdr agent focus <target>` accepts pane ids, but only resolves panes that have an agent.
  Verified against 0.7.4: a plain shell pane returns `agent_not_found`.

`pane.focus` with a `pane_id` exists in the socket API and works on any pane.

Two things that looked like costs turned out not to be. The protocol is trivial — connect,
write one JSON line, read one JSON line, and the server closes the connection — so the client
is shorter than the argv-building and stdout/stderr-parsing the CLI would need. And the
version coupling is not new: the CLI returns the same JSON shapes, so a response that changes
breaks both transports equally.

## Consequences

- Every herdr call goes through one connection. There is no pooling to get wrong.
- `SocketHerdr::from_env` fails with an explanation when `HERDR_SOCKET_PATH` is unset, which
  is what happens if someone runs the binary from a shell instead of letting herdr launch it.
- The wire types are permissive — unknown fields ignored, optional fields defaulted — so a
  newer herdr that adds fields keeps parsing.
- This is a Unix-socket client, which is part of why Windows is out of scope for v1.
