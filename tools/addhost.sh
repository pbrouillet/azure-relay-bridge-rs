#!/bin/bash
# Adds an IP address with the given hostname to /etc/hosts.
# Usage: sudo ./addhost.sh <ipaddress> <hostname>

set -e

if [ $# -ne 2 ]; then
    echo "Usage: sudo $0 <ipaddress> <hostname>"
    exit 1
fi

IP="$1"
HOSTNAME="$2"
HOSTS="/etc/hosts"

# Remove existing entry if present
if grep -qE "^\s*\S+\s+${HOSTNAME}\s*$" "$HOSTS"; then
    echo "Updating existing entry for '$HOSTNAME'..."
    sed -i.bak "/^\s*\S\+\s\+${HOSTNAME}\s*$/d" "$HOSTS"
fi

# Add new entry
echo -e "${IP}\t${HOSTNAME}" >> "$HOSTS"
echo "Added: ${IP} -> ${HOSTNAME}"
