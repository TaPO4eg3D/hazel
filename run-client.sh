# A small convinience script to bind the client on a hotkey (for quick layout development)

killall -q hazel_client
cd /home/tapo4eg3d/Projects/hazel-client2/ || exit 1

BUILD_LOG=$(cargo build 2>&1)
STATUS=$?

if [ $STATUS -eq 0 ]; then
    ./target/debug/hazel_client
else
    echo "$BUILD_LOG"
    notify-send "Hazel Client Build Failed" "$(echo "$BUILD_LOG" | tail -n 5)"
fi
