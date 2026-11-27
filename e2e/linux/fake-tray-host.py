#!/usr/bin/env python3
"""Zone de notification factice pour background.sh : publie org.kde.StatusNotifierWatcher sur le bus de session avec un
hôte enregistré. Sans elle, Cubelink voit qu'aucune icône ne serait visible (Xvfb + openbox n'ont pas de zone de
notification) et, comme prévu, fermer la fenêtre quitte au lieu de masquer : le scénario « en fond » ne serait pas testé.
Usage : python3 fake-tray-host.py (tourne jusqu'à être tué). Prérequis : python3-gi."""
from gi.repository import Gio, GLib

XML = """<node><interface name="org.kde.StatusNotifierWatcher">
  <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
  <method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method>
  <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
  <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
  <property name="ProtocolVersion" type="i" access="read"/>
  <signal name="StatusNotifierItemRegistered"><arg type="s"/></signal>
  <signal name="StatusNotifierHostRegistered"/>
</interface></node>"""
ITEMS = []


def on_call(_conn, sender, _path, _iface, method, params, invocation):
    if method == "RegisterStatusNotifierItem":
        ITEMS.append(params.unpack()[0] if params.unpack()[0].startswith(":") else sender)
    invocation.return_value(None)


def on_get(_conn, _sender, _path, _iface, prop):
    return {
        "RegisteredStatusNotifierItems": GLib.Variant("as", ITEMS),
        "IsStatusNotifierHostRegistered": GLib.Variant("b", True),
        "ProtocolVersion": GLib.Variant("i", 0),
    }[prop]


def on_bus(conn, _name):
    info = Gio.DBusNodeInfo.new_for_xml(XML).interfaces[0]
    conn.register_object("/StatusNotifierWatcher", info, on_call, on_get, None)


Gio.bus_own_name(Gio.BusType.SESSION, "org.kde.StatusNotifierWatcher", Gio.BusNameOwnerFlags.NONE, on_bus, None, None)
GLib.MainLoop().run()
