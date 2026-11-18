fn main() {
    // CONSTRAINT: WebRTC's Objective-C categories are in a static archive and otherwise
    // stripped, causing missing selectors when the publisher starts on macOS.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bin=hebbian-robot-use-media=-ObjC");
    }
}
