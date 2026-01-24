# A small convinience script to bind the client on a hotkey (for quick layout development)

PROFILE=${1:-default}

if [[ $PROFILE == "default" ]]; then
    killall -q hazel_client
fi

cd /home/tapo4eg3d/Projects/hazel/crates/client/ || exit 1

BUILD_LOG=$(cargo build 2>&1)
STATUS=$?

echo $PROFILE;

if [ $STATUS -eq 0 ]; then
    ../../target/debug/hazel_client --profile $PROFILE
else
    echo "$BUILD_LOG"
    notify-send "Hazel Client Build Failed" "$(echo "$BUILD_LOG" | tail -n 5)"
fi
