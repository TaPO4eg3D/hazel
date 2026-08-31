client-flamegraph:
    RUSTFLAGS="-Clink-arg=-Wl,--no-rosegment -C force-frame-pointers=yes" \
        cargo flamegraph -c "record -F 997 --call-graph fp -g" -p client --profile dev -- --profile test
