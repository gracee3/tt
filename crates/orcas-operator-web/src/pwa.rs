pub fn register_service_worker() {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let navigator = window.navigator();
            if let Some(service_worker) = navigator.service_worker() {
                let _ = service_worker.register("/sw.js");
            }
        }
    }
}
