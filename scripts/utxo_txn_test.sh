#!/usr/bin/env bash

args=$*
cmd="$(echo "$args" | awk '{print $1}')"

#full00="http://dev-qa01-us-west-2-full-000-open.dev.findora.org"
#full01="http://dev-qa01-us-west-2-full-001-open.dev.findora.org"
#mynode="http://34.211.109.216"
#endpoint="http://34.211.109.216"
endpoint="https://dev-qa01.dev.findora.org"
port=26657

script_dir=$(dirname "$0")
. "$script_dir"/common.sh

latest_height=$(curl -s "$endpoint:$port/status" | jq -r .result.sync_info.latest_block_height)

echo "latest height:$latest_height"

gen_one_key() {
    kp=$(fn genkey | grep -E "pub_key|sec_key")
    pk=$(echo "$kp" | grep "pub_key" | awk -F'"' '{print $4}')
    sk=$(echo "$kp" | grep "sec_key" | awk -F'"' '{print $4}')
    echo "$pk" "$sk"
}

gen_source_keys() {
    count=$1
    if ! mkdir fra_source_keys; then
        echo "Please do a little check"
        exit 1
    fi
    while ((count > 0)); do
        kp=$(gen_one_key)
        pk=$(echo "$kp" | awk '{print $1}')
        sk=$(echo "$kp" | awk '{print $2}')
        echo -n "$sk" >>fra_source_keys/sk."$count"
        echo -n "$pk" >>fra_source_keys/pk."$count"
        echo "$pk" >>fra_source_keys/pks
        ((count -= 1))
    done
}

gen_pub_keys() {
    cnt=$1
    file=$2
    if [ -f "$file" ]; then
        echo "$file exits"
        exit 1
    fi
    while ((cnt > 0)); do
        pk=$(gen_one_key | awk '{print $1}')
        echo "$pk" >>"$file"
        ((cnt -= 1))
    done
    echo "Generated $(line_of "$file") public keys in $(basename "$file")"
}

deposit_source_keys() {
    amount=$1
    pks="fra_source_keys/pks"
    if [ ! -f "$pks" ]; then
        echo "source keys not exits"
        exit 1
    fi

    echo "Deposit $(line_of $pks) source keys with $amount FRA each"

    fn transfer-batch -t $pks --amount "$amount" >/dev/null
}

do_seq_transfers() {
    sk=$1
    file=$2

    while read -r to; do
        fn transfer -f "$sk" -t "$to" --amount 1 >/dev/null
        sleep 5
    done <"$file"
}

run_a_test() {
    count=$1
    if [ ! -d fra_source_keys ]; then
        exit 1
    fi
    sks=$(ls fra_source_keys/ | grep -E "sk.[0-9]+$")
    for sk in $sks; do
        sk="fra_source_keys/$sk"
        rm -rf "$sk.target_keys"
        targets="$sk.target_keys"
        gen_pub_keys "$count" "$targets"
        fn transfer-batch -f "$sk" -t "$targets" --amount 1 2>&1 >/dev/null &
        #do_seq_transfers "$sk" "$targets"
    done
}

show_source_balance() {
    if [ ! -d fra_source_keys ]; then
        exit 1
    fi
    sks=$(ls fra_source_keys/ | grep -E "sk.[0-9]+$")
    for sk in $sks; do
        sk="fra_source_keys/$sk"
        balance=$(fn wallet --show --asset fra --seckey "$sk" | grep FRA)
        echo "$(basename "$sk") $balance"
    done
}

usage() {
    echo "$0 gen_source_keys COUNT"
    echo "$0 deposit_source_keys AMOUNT"
    echo "$0 gen_target_keys COUNT"
    echo "$0 run_test COUNT"
    echo "$0 show"
}

myName=$(basename "$0")

if [ -e "$cmd" ]; then
    usage "$myName"
elif [ "$cmd" == "gen_source_keys" ]; then
    count="$(echo "$args" | awk '{print $2}')"
    gen_source_keys "$count"
elif [ "$cmd" == "show" ]; then
    show_source_balance
elif [ "$cmd" == "gen_target_keys" ]; then
    count="$(echo "$args" | awk '{print $2}')"
    gen_pub_keys "$count" fra_target_keys
elif [ "$cmd" == "deposit_source_keys" ]; then
    amount="$(echo "$args" | awk '{print $2}')"
    deposit_source_keys "$amount"
elif [ "$cmd" == "run_test" ]; then
    count="$(echo "$args" | awk '{print $2}')"
    run_a_test "$count"
else
    echo "current args: $args"
    usage "$myName"
fi
