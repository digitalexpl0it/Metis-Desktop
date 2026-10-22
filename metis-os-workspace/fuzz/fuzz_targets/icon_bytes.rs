#![no_main]
use libfuzzer_sys::fuzz_target;
use metis_remote::validate_icon_bytes;

fuzz_target!(|data: &[u8]| {
    let _ = validate_icon_bytes(data);
});
