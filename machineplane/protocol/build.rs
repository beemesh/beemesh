use std::process::Command;
use std::path::Path;

fn main() {
    // Where the .fbs schema files live
    let schema_dir = Path::new("schema/machine");

    // Ensure schema dir exists
    if !schema_dir.exists() {
        println!("cargo:warning=No schema directory found at {:?}, skipping flatc generation", schema_dir);
        return;
    }

    // Check flatc availability
    let mut flatc_path = "flatc";
    let mut flatc_ok = Command::new("flatc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !flatc_ok {
        flatc_path = "../flatc";
        flatc_ok = Command::new(flatc_path).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    }

    if !flatc_ok {
        println!("cargo:warning=flatc not found on PATH; skipping flatbuffer code generation");
        return;
    }

    let mut generated_any = false;

    // Iterate over .fbs files in schema/ and invoke flatc
    for entry in std::fs::read_dir(schema_dir).expect("read schema dir") {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => { println!("cargo:warning=failed to read schema entry: {}", e); continue; }
        };
        let path = entry.path();
        if path.extension().map(|s| s == "fbs").unwrap_or(false) {
            // Generate Rust code into src/generated
            let out_dir = Path::new("src/generated");
            std::fs::create_dir_all(out_dir).expect("create generated dir");

            let status = Command::new(flatc_path)
                .arg("--rust")
                .arg("--gen-onefile")
                .arg("-o")
                .arg(out_dir)
                .arg(&path)
                .status();

            match status {
                Ok(s) if s.success() => {
                    generated_any = true;
                }
                Ok(s) => {
                    println!("cargo:warning=flatc failed for {:?} with status: {}", path, s);
                }
                Err(e) => {
                    println!("cargo:warning=failed to run flatc for {:?}: {}", path, e);
                }
            }
        }
    }

    if generated_any {
        println!("cargo:rustc-cfg=has_flatbuffers_generated");
    } else {
        println!("cargo:warning=no flatbuffers code generated; tests relying on generated code will be skipped");
    }

    // Re-run build.rs if any schema files change
    println!("cargo:rerun-if-changed=schema");
}
