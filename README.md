# SteamOS Manager

SteamOS Manager is a small daemon whose primary purpose is to give Steam
something with a DBus API that runs as root and can do things to the system
without having to have hard coded paths and DBus API in the Steam client
itself. It also runs some background tasks, as well as giving a centralized
daemon for managing some user-context tasks that aren't particularly related to
Steam itself.

Many of these root tasks are implemented as athin wrapper around other scripts.
This lets the DBus APIs invoke the scripts as a privileged but non-root user
and provides feedback for each DBus method it implements.

## How it works

The SteamOS Manager runs as `root` and exposes a DBus API on the system bus.
Another instance runs as `deck` and exposes a DBus API on the session bus with
a few extra methods that handle tasks that are user-specific. These methods run
directly in the user daemon as that user. All other APIs are relayed to the
root daemon via its DBus API.

## To add a new method

To add a new method that doesn't require root privileges, add it to the user
daemon's DBus API directly. This is implemented in `src/manager/usr.rs`

To add a new method that does require root privileges, add it to the root
daemon's DBus API in `src/manager/root.rs`, update the proxy implementation in
`src/proxy.rs`, and add a relay method in `src/manager/user.rs`.

In both cases, make sure to add the new API to the XML schema. The methods are
test automatically to match the schema, so these tests will fail if they don't.

Further, if this is the first change made to the schema since the last tag,
increment the API version number in `src/lib.rs`.
