fn main() {
    #[cfg(target_os = "windows")]
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR");
        let icon_path = std::path::Path::new(&manifest_dir).join("assets/icons/app-icon.ico");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(icon_path.to_string_lossy().as_ref());
        resource
            .compile()
            .expect("failed to compile Windows icon resource");
    }
}
