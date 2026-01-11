use capture::audio::linux;


fn main() {
    linux::Audio::new()
        .unwrap();
}
