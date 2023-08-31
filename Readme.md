This is the SteamOS Manager.

It is a small daemon that's sole purpose is to give Steam something with a dbus
api that runs as root and can do things to the system without having to have
hard coded paths and dbus apis in the Steam client itself.

It is a thin wrapper around other scripts and dbus apis that provides feedback for
each dbus method it implements.

How it works:

The SteamOS Manager reads various config files (one per thing that it needs to support)
that have a DBus method defined in them along with the parameters needed for that method.
When it reads each config file it creates a DBus method on it's interface that other applications
and processes can call. When each method is called, either the script referred to in the config
is called and output and exit codes are given back to the caller, or a dbus method is called
from another daemon and the feedback it gets is given back to the caller.

To add a new method:

Add a new config file (more on this later once we flesh out details) with the method name, parameters
and what should happen when it is called. For most dbus type methods, the incoming method and parameters
will likely closely resemble what we call on another daemon. The purpose of this is to make it flexible
for various operating systems to do things their own way. For example on a system that doesn't want to
include systemd many systemd type operations could be carried out by scripts instead of calling a systemd
dbus api.
