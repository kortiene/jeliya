use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=android/backup_rules.xml");
    println!("cargo:rerun-if-changed=android/data_extraction_rules.xml");

    // dx 0.7.9 generates its Gradle resources before invoking Cargo and has no
    // documented hook for custom res/xml files. Copy the two backup-exclusion
    // resources into the generated project while Cargo is building the real
    // Android artifact. WRY_ANDROID_KOTLIN_FILES_OUT_DIR is set by dx itself;
    // deriving from Cargo OUT_DIR is wrong because that is a separate tree.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android") {
        return;
    }

    let kotlin = PathBuf::from(
        std::env::var_os("WRY_ANDROID_KOTLIN_FILES_OUT_DIR")
            .expect("dx did not set WRY_ANDROID_KOTLIN_FILES_OUT_DIR"),
    );
    let main = kotlin
        .ancestors()
        // The Kotlin package itself also ends in `.../dev/dioxus/main`; name
        // matching therefore stages resources under Kotlin and looks green
        // until AAPT. The actual Android src/main is the ancestor that owns
        // the generated manifest.
        .find(|path| path.join("AndroidManifest.xml").is_file())
        .expect("generated Android src/main not found from WRY Kotlin directory");
    let destination = main.join("res/xml");
    std::fs::create_dir_all(&destination).expect("could not create generated res/xml");

    for name in ["backup_rules.xml", "data_extraction_rules.xml"] {
        std::fs::copy(PathBuf::from("android").join(name), destination.join(name))
            .unwrap_or_else(|error| panic!("could not stage {name}: {error}"));
    }
}
