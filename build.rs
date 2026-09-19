//! Embeds the app icon (and basic version info) into the compiled Windows
//! executable itself, via a Windows resource (.res) file linked into the
//! binary. This is a separate concern from `cargo packager`'s `icons`
//! config in Cargo.toml, which only sets the icon shown on the *installer*
//! (the .msi/setup.exe) and the Start Menu shortcut it creates -
//! `cargo packager` never touches the compiled binary's own PE resources.
//! Without this, the installed app's own .exe (as seen in the taskbar,
//! Alt-Tab, "Open File Location", Add/Remove Programs, etc.) shows
//! Windows' generic executable icon no matter what icon the installer uses.
//!
//! No-op on every other platform/target - resource-compiling only makes
//! sense when actually targeting Windows.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/abyssal-cdg-icon.ico");
        // winresource defaults ProductName to the crate's Cargo.toml name
        // ("abyssal-cdg") - override it to the actual display name used
        // everywhere else in the app (window title, packager config).
        res.set("ProductName", "Abyssal CDG Creator");
        res.set(
            "FileDescription",
            "Turns an audio file and pasted lyrics into a real karaoke .cdg file",
        );
        if let Err(e) = res.compile() {
            // A build.rs failure here would break `cargo build` on Windows
            // entirely over what's ultimately a cosmetic icon issue -
            // surface it as a warning instead of a hard build error.
            println!("cargo:warning=failed to embed Windows icon/version resource: {e}");
        }
    }
}
