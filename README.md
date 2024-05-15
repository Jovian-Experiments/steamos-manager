This is the SteamOS Manager.

It is a small daemon that's sole purpose is to give Steam something with a DBus
api that runs as root and can do things to the system without having to have
hard coded paths and DBus apis in the Steam client itself.

It is a mostly thin wrapper around other scripts and DBus apis that provides feedback for
each DBus method it implements.

How it works:

The SteamOS Manager runs as root and exposes a DBus api on the system bus.
Another instance runs as deck user and exposes a DBus api on the session bus with
a few extra methods. These methods run directly in the user daemon as deck user. All
other apis forward to the root daemon via it's DBus api.

To add a new method:

To add a new method that doesn't require root privileges add it to the user daemon's DBus api
directly.

To add a new method that does require root privileges add it to the root daemon's DBus api and it will
be exposed automatically in the user daemon.
