fn main() {
    println!("cargo:rerun-if-env-changed=TSHELL_UPDATE_REPOSITORY");
    println!("cargo:rerun-if-changed=assets/tshell.rc");
    println!("cargo:rerun-if-changed=assets/tshell.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/tshell.rc", embed_resource::NONE)
            .manifest_required()
            .expect("could not embed TShell icon");
    }
}
