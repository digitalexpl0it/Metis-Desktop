//! Link FreeRDP shadow server and compile the Metis portal-frame subsystem.

fn main() {
    let freerdp = pkg_config::Config::new()
        .atleast_version("3.0")
        .probe("freerdp-shadow3")
        .expect("freerdp-shadow3.pc missing — install freerdp3-dev");
    let _ = pkg_config::Config::new()
        .atleast_version("3.0")
        .probe("freerdp3")
        .expect("freerdp3.pc missing — install freerdp3-dev");
    let _ = pkg_config::Config::new()
        .atleast_version("3.0")
        .probe("winpr3")
        .expect("winpr3.pc missing — install libwinpr3-dev");

    let mut build = cc::Build::new();
    build.file("c/metis_shadow.c");
    build.include("c");
    for path in &freerdp.include_paths {
        build.include(path);
    }
    // FreeRDP / WinPR headers mark legacy typedefs and callbacks deprecated on
    // include (NTLM/Kerberos V1, codecs_free, pVerifyCertificate, …). Silence
    // that noise so cargo build isn't flooded — Metis doesn't call those APIs.
    build.flag_if_supported("-Wno-unused-parameter");
    build.flag_if_supported("-Wno-deprecated-declarations");
    build.compile("metis_shadow");

    println!("cargo:rerun-if-changed=c/metis_shadow.c");
    println!("cargo:rerun-if-changed=c/metis_shadow.h");
}
