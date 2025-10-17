use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn generate_mobile_bindings() {
    println!("cargo:rerun-if-changed=src/mobile/wrapper.h");

    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    let mut builder = bindgen::Builder::default()
        .header("src/mobile/wrapper.h")
        .clang_arg("-Wno-everything")
        .layout_tests(false);

    if os == "android" {
        if let Ok(ndk_home) = env::var("NDK_HOME") {
            let host_os = env::consts::OS;
            let host_tag = match host_os {
                "macos" => "darwin-x86_64",
                "linux" => "linux-x86_64",
                "windows" => "windows-x86_64",
                _ => panic!("unsupported host os for android build!"),
            };
            let include_path = PathBuf::from(&ndk_home)
                .join("toolchains")
                .join("llvm")
                .join("prebuilt")
                .join(host_tag)
                .join("sysroot")
                .join("usr")
                .join("include");
            builder = builder.clang_arg(format!("-I{}", include_path.to_string_lossy()));

            // Also add the target-specific include path, e.g. for aarch64-linux-android
            let target = env::var("TARGET").unwrap();
            let target_include_path = include_path.join(&target);
            if target_include_path.exists() {
                builder = builder.clang_arg(format!("-I{}", target_include_path.to_string_lossy()));
            }

            // Set the target for clang
            builder = builder.clang_arg(format!("--target={}", target));
        } else {
            panic!("NDK_HOME is not set for android build!");
        }
    } else if os == "ios" {
        let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
        if arch == "aarch64" {
            // https://github.com/rust-lang/rust-bindgen/issues/1211
            builder = builder.clang_arg("--target=arm64-apple-ios");
            // sdk path find by `xcrun --sdk iphoneos --show-sdk-path`
            let output = Command::new("xcrun")
                .arg("--sdk")
                .arg("iphoneos")
                .arg("--show-sdk-path")
                .output()
                .expect("failed to execute xcrun");
            let inc_path =
                Path::new(String::from_utf8_lossy(&output.stdout).trim()).join("usr/include");
            builder = builder.clang_arg(format!(
                "-I{}",
                inc_path.to_str().expect("invalid include path")
            ));
        }
    }

    let bindings = builder
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("mobile_bindings.rs"))
        .expect("Couldn't write bindings!");
}

fn main() {
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    if os == "ios" || os == "macos" || os == "android" {
        generate_mobile_bindings();
    }

    if env::var("PROTO_GEN").is_ok() {
        // println!("cargo:rerun-if-changed=src/config/internal/config.proto");
        protobuf_codegen::Codegen::new()
            .out_dir("src/config/internal")
            .includes(["src/config/internal"])
            .inputs(["src/config/internal/config.proto"])
            .customize(
                protobuf_codegen::Customize::default()
                    .generate_accessors(false)
                    .gen_mod_rs(false)
                    .lite_runtime(true),
            )
            .run()
            .expect("Protobuf code gen failed");

        // println!("cargo:rerun-if-changed=src/config/geosite.proto");
        protobuf_codegen::Codegen::new()
            .out_dir("src/config")
            .includes(["src/config"])
            .inputs(["src/config/geosite.proto"])
            .customize(
                protobuf_codegen::Customize::default()
                    .generate_accessors(false)
                    .gen_mod_rs(false)
                    .lite_runtime(true),
            )
            .run()
            .expect("Protobuf code gen failed");

        protobuf_codegen::Codegen::new()
            .out_dir("src/app/outbound")
            .includes(["src/app/outbound"])
            .inputs(["src/app/outbound/selector_cache.proto"])
            .customize(
                protobuf_codegen::Customize::default()
                    .generate_accessors(false)
                    .gen_mod_rs(false)
                    .lite_runtime(true),
            )
            .run()
            .expect("Protobuf code gen failed");
    }

    // ROG protobuf generation
    if std::env::var("CARGO_FEATURE_OUTBOUND_ROG").is_ok() {
        if std::path::Path::new("src/proxy/rog/proto/rog.proto").exists() {
            tonic_prost_build::compile_protos("src/proxy/rog/proto/rog.proto");
        }
    }
}
