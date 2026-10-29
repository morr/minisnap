fn main() {
    // ImageIO for PNG encoding (see output module).
    println!("cargo:rustc-link-lib=framework=ImageIO");
    // Accessibility trust check (AXIsProcessTrustedWithOptions) for the
    // permission module.
    println!("cargo:rustc-link-lib=framework=ApplicationServices");
}
