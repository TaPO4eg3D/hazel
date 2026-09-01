client-profile:
    RUSTFLAGS="-Clink-arg=-Wl,--no-rosegment -C force-frame-pointers=yes" \
        cargo flamegraph -c "record -F 997 --call-graph fp -g" -p client --profile dev -- --profile test

simulation-profile:
    RUSTFLAGS="-Clink-arg=-Wl,--no-rosegment -C force-frame-pointers=yes" \
        cargo flamegraph -c "record -F 997 --call-graph fp -g" -p simulation --profile dev -- \
        screen-cast --mode file --file /home/tapo4eg3d/Downloads/test.mov
