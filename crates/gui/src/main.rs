slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    MainWindow::new()?.run()
}
