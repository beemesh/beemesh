#!/bin/sh

PORT=$(shuf -i 2000-65000 -n 1)

echo "*** Startup $0 suceeded now starting service using eval to expand CMD variables ***"
exec $(eval echo "$@")
