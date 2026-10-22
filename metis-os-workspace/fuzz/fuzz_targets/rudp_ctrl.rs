#![no_main]
use libfuzzer_sys::fuzz_target;
use metis_protocol::try_decode_rudp_frame;

fuzz_target!(|data: &[u8]| {
    let _ = try_decode_rudp_frame(data);
});
