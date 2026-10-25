// SQLx migrate! on stable Rust requires a build-script to watch new migrations.
fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
