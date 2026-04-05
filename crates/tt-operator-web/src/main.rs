#![allow(warnings)]

#[cfg(target_arch = "wasm32")]
fn main() {
    tt_operator_web::mount_app();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("tt-operator-web is intended to run in a browser via trunk.");
}
