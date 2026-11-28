#!/bin/sh
# Smoke test of a Debian package in a clean Debian container (run by scripts/build-linux.sh
# smoke): installs /out/<deb>, starts fxfm on a virtual screen (Xvfb) with an empty HOME and
# saves a screenshot and the output as /out/smoke-<arch>.png / .log.
#   sh smoke.sh fxfm_0.1.25_arm64.deb arm64
set -e
DEB=$1
ARCH=$2
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq xvfb imagemagick dbus-x11 fonts-dejavu-core >/dev/null
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "/out/$DEB" >/dev/null
HOME=$(mktemp -d)
export HOME
Xvfb :99 -screen 0 1280x800x24 >/dev/null 2>&1 &
export DISPLAY=:99
sleep 2
dbus-launch fxfm >"/out/smoke-$ARCH.log" 2>&1 &
sleep 15
import -window root "/out/smoke-$ARCH.png"
pgrep -x fxfm >/dev/null && echo "fxfm is running" || { echo "fxfm is not running"; exit 1; }
