fn main() {
    // ImageIO for PNG encoding (see output module).
    println!("cargo:rustc-link-lib=framework=ImageIO");
}
