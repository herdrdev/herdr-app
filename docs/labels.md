# Shared state in labels

Herdr clients (the desktop app, the phone app, a terminal UI) share two
things through herdr-core terminal labels: which agents a person has seen,
and which are pinned. Look at an agent on one client and it stops being
"done" on all of them; pin it on one and it's pinned everywhere.

Labels live on the terminal, so this state ends with it. Every client
already gets them: `events.subscribe` snapshots carry `labels` and `turn`,
and each `labels` event carries the whole map. Remote terminals
(`mac/t7`) work the same way.

Clients that don't know these labels keep working: everything here is
optional, and a missing label falls back to what the client did before.

## `herdr.seen=<turn>`: the last turn a person looked at

`turn` is core's counter: it goes up each time the terminal stops working.

Showing an agent:

1. `blocked` or `error`: needs you.
2. `working`: working.
3. Otherwise (`idle`, `done`, ...): **done** when `turn > herdr.seen`,
   else **idle**.

With no `herdr.seen` label, use your own seen rules for that terminal.
A missing label is not `0`.

Writing it:

- When a person is looking at an agent, write `herdr.seen=<turn>`. Write
  only to raise it, and also when it's missing (so terminals started
  elsewhere join).
- "Looking" means really in front of someone: the window or app in the
  foreground and that agent drawn on screen (a peek or a floating view
  counts; a pane covered by another terminal doesn't). A screen left on in an empty room
  shouldn't clear everyone's "done".
- Write once per turn while it stays on screen, not on every update: then
  a "mark unread" from another client isn't overruled by a screen nobody
  watches. Looking again later raises it again.
- A turn ends with a `status` event first, then a `turn` event. Decide on
  the `turn` event.
- Mark unread is the one lower write: `herdr.seen=<turn - 1>`. At turn 0
  nothing has finished yet, so there is nothing to mark.
- A terminal a client starts gets `herdr.seen=0`.

Last writer wins. Turn counts observed progress: an agent that goes from
blocked to idle without working again keeps its turn.

How long a done row stays in place while you look at it is up to each
client.

## `herdr.pinned=1`: pinned

Present and non-empty: pinned. Unpin by writing it empty
(`herdr.pinned=`), which removes it. Pinned agents go first, in the
client's usual order.

In the desktop app, mods pin with `terminal.pin {terminal, pinned}` and
read `pinned` from `agents()`; they never write the label themselves.

## Adding more

Labels suit small values that change at human speed. Use the `herdr.`
prefix. Bigger or app-wide state belongs in documents (`doc.get`,
`doc.set`), and state for one device stays on that device.
