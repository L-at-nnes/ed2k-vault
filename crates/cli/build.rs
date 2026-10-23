fn main() {
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/icon.ico");
        resource.compile().expect("cannot embed the Windows icon");
    }
}
