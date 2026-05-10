fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/tomo.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/tomo.proto");
    Ok(())
}
