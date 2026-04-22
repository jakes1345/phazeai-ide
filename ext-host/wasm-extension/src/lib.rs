#[link(wasm_import_module = "phazeai")]
extern "C" {
    fn log(ptr: *const u8, len: usize);
}

pub fn phazeai_log(msg: &str) {
    unsafe {
        log(msg.as_ptr(), msg.len());
    }
}

#[no_mangle]
pub extern "C" fn activate() {
    phazeai_log("WASM Extension Activated!");
}

#[no_mangle]
pub extern "C" fn execute_command() {
    phazeai_log("WASM Extension Executed Command!");
}
