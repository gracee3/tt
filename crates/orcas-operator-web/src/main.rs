#[cfg(target_arch = "wasm32")]
fn main() {
    orcas_operator_web::mount_app();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("orcas-operator-web is intended to run in a browser via trunk.");
}
