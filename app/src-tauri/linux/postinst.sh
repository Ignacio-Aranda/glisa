#!/bin/sh
# Apply the udev rule that gives the logged-in user access to Logitech HID devices and
# /dev/uinput (installed to /usr/lib/udev/rules.d/70-glisa.rules).
set -e
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules || true
    udevadm trigger --subsystem-match=hidraw --action=change || true
    # The mice's input nodes: Flow reads whether a button is held there.
    udevadm trigger --subsystem-match=input --action=change || true
    udevadm trigger --subsystem-match=misc --sysname-match=uinput --action=change || true
fi
modprobe uinput 2>/dev/null || true
exit 0
