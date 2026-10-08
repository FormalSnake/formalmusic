fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/windows/formalmusic.ico");
        // Task Manager and the volume mixer name the process by this.
        res.set("FileDescription", "FormalMusic");
        res.set("ProductName", "FormalMusic");
        res.compile().expect("embed the Windows icon");
    }
}
