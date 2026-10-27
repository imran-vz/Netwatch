#!/usr/bin/env bash
# Remove Utilify's fan-control helper (the root LaunchDaemon install.sh
# adds). Stopping it returns the fans to automatic. Utilify keeps working,
# with fans and temperatures read-only.
#
#   scripts/uninstall-fan-helper.sh
set -euo pipefail

LABEL="dev.utilify.fanhelper"

printf '\033[1m==>\033[0m %s\n' "Removing the fan helper ($LABEL); sudo asks for your password"
sudo /bin/bash -euc '
  launchctl bootout "system/$1" 2>/dev/null || true
  rm -f "/Library/LaunchDaemons/$1.plist" \
        "/Library/PrivilegedHelperTools/$1" \
        "/var/run/$1.sock" "/var/run/$1.state" \
        /var/log/utilify-fanhelper.log
' _ "$LABEL"
printf '\033[1m==>\033[0m %s\n' "Fan helper removed"
