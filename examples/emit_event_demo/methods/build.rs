fn main() {
    println!("cargo:rerun-if-changed=../../../nssa/core/src/program.rs");
    println!("cargo:rerun-if-changed=../../../lez-events/src/lib.rs");
    risc0_build::embed_methods();
}
