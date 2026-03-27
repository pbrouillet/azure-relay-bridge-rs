#!/bin/bash
# Removes a hostname entry from /etc/hosts.
# Usage: sudo ./removehost.sh <hostname>

set -e

if [ $# -ne 1 ]; then
    echo "Usage: sudo $0 <hostname>"
    exit 1
fi

HOSTNAME="$1"
HOSTS="/etc/hosts"

if grep -qE "^\s*\S+\s+${HOSTNAME}\s*$" "$HOSTS"; then
    sed -i.bak "/^\s*\S\+\s\+${HOSTNAME}\s*$/d" "$HOSTS"
    echo "Removed entry for '$HOSTNAME'"
else
    echo "No entry found for '$HOSTNAME'"
fi
