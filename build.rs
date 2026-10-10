//! The icon and version information Windows shows for the executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    #[cfg(windows)]
    {
        if std::path::Path::new("assets/app.ico").exists() {
            let mut resource = winresource::WindowsResource::new();
            resource
                .set_icon("assets/app.ico")
                .set("ProductName", "Native YTM")
                .set("FileDescription", "Native YTM, a YouTube Music client")
                .set("LegalCopyright", "Personal use");
            if let Err(error) = resource.compile() {
                println!("cargo:warning=no Windows resources: {error}");
            }
        }
    }
}
