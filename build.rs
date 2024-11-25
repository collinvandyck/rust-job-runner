use std::fs;

// generate the protobufs. tonic-build will automatically output the rerun-if-changed cargo
// directives
fn main() {
    let out_dir = "src/pb";
    let includes = &["proto"];
    fs::create_dir_all(out_dir).unwrap();
    let protos = fs::read_dir("proto")
        .unwrap()
        .filter_map(Result::ok)
        .map(|f| f.path())
        .collect::<Vec<_>>();
    tonic_build::configure()
        .out_dir(out_dir)
        .compile_protos(&protos, includes)
        .unwrap();
}
