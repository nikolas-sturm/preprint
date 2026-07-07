fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("packaging/preprint.ico");
        res.set("FileDescription", "Prepare photo files for printing");
        res.set("ProductName", "Preprint");
        if let Err(error) = res.compile() {
            println!("cargo:warning=failed to embed Windows icon: {error}");
        }
    }
}
