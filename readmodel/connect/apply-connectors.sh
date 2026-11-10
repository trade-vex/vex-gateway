#!/usr/bin/env bash
# Makes the Kafka Connect cluster at the given REST URL run exactly the connectors in
# connectors/: creates or updates each one from its file (PUT /connectors/<file name>/config) and
# deletes every connector that has no file. Applying twice changes nothing.
#
#   readmodel/connect/apply-connectors.sh http://connect:8083
set -euo pipefail

connect_url=${1:?usage: apply-connectors.sh <Kafka Connect REST URL>}
connectors_dir=$(cd "$(dirname "$0")/connectors" && pwd)

# Prints the response body: on stdout when the request succeeds, on stderr when it fails. Connect
# answers 409 Conflict while its workers rebalance, which applying connectors itself causes, so a
# request refused with 409 is tried again after 1, 2, 4, 8 and 16 s: at most 6 times in all.
# Any other failure fails at once.
call() {
    local attempt response code delay=1
    for attempt in 1 2 3 4 5 6; do
        if response=$(curl --silent --show-error --fail-with-body \
            --write-out '\n%{http_code}' "$@"); then
            echo "${response%$'\n'*}"
            return 0
        fi
        code=${response##*$'\n'}
        if [ "$code" != 409 ] || [ "$attempt" = 6 ]; then
            echo "${response%$'\n'*}" >&2
            return 1
        fi
        echo "Connect is rebalancing (409); trying again in $delay s" >&2
        sleep "$delay"
        delay=$((delay * 2))
    done
}

for file in "$connectors_dir"/*.json; do
    name=$(basename "$file" .json)
    call -X PUT -H 'Content-Type: application/json' --data-binary @"$file" \
        "$connect_url/connectors/$name/config" > /dev/null
    echo "applied $name"
done

# GET /connectors answers a JSON array of names: ["pg-balances","pg-orders",...].
names=$(call "$connect_url/connectors")
for name in $(echo "$names" | tr -d '[]"' | tr ',' ' '); do
    if [ ! -f "$connectors_dir/$name.json" ]; then
        call -X DELETE "$connect_url/connectors/$name" > /dev/null
        echo "deleted $name"
    fi
done
