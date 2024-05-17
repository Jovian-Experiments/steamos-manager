# SteamOS Manager

SteamOS Manager is a system daemon that aims to abstract steam's
interactions with the operating system. The goal is to have a standardized
interface so that SteamOS specific features in the Steam client, e.g. TDP
management, can be exposed in any linux distro that provides an implementation
of this DBus API.

The interface may be fully or partially implemented. The Steam client will
check which which features are available at startup and restrict the settings
it presents to the user based on feature availability.

Some of the features that SteamOS Manager enables include:
- GPU clock management
- TDP management
- BIOS/Dock updates
- Storage device maintenance tasks
- External storage device formatting
  * Steam geneally performs device enumeration via UDisks2, but formatting happens via SteamOSManager

For a full list of features please refer to the [interface specification](https://gitlab.steamos.cloud/holo/steamos-manager/-/blob/master/com.steampowered.SteamOSManager1.xml).

Other notable dbus interfaces used by the Steam Client include:
  * org.freedesktop.UDisks2
  * org.freedesktop.portal.desktop
  * org.freedesktop.login1
  * org.bluez


## Interface compatibility notes

SteamOS Manager and the Steam client are normally updated independently of
each other. So the interface must remain binary compatible across releases.

In general, when making changes to the interface please consider the following:
- Method signatures must not be altered
  * Instead, prefer exposing a new symbol and add a compatibility adapter to the previous interface
- Changes in behaviour should be avoided
  * Consider how a change would affect the beta and stable release of the Steam client
- Features must have a mechanism to discover if they are available or not
  * E.g. for a feature exposed as a property if the property is not present on the bus it means the feature is unsupported.

Note that while SteamOS Manager must never break binary compatibility of
the interface, the Steam client makes no guarantees that older versions of
an interface will be used if available. As a rule of thumb, the client will
always provide full support for the SteamOS Manager interface version available
in the Stable release of SteamOS.


## Implementation details

SteamOS Manager is compromised of two daemons, one runs as the logged in user
and exposes a public DBus API on the session bus. And the second daemon runs as
the `root` user and exposes a limited DBus API on the system bus for tasks
that require elevated permissions to execute.

The DBus API exposed on the system bus is considered a private implementation
detail of SteamOS Manager and it may be changed at any moment and without warning.
For this reason, we don't provide an XML schema for the system daemon's interface.

## Extending the API

To extend the API with a new method or property first update the XML schema.
Then extend the user daemon's DBus API which is implemented in `src/manager/usr.rs`.

If the new functionality requires elevated privileges, then extend the system
daemon's DBus API in `src/manager/root.rs` with the necessary helpers to complete
the task. Please consider keeping as much logic as possible in the user context.

