fn main() {
    if std::env::var("DOCS_RS").is_ok() {
        return;
    }

    println!("cargo:rerun-if-changed=include/");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/nix.cpp");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/hash.rs");
    println!("cargo:rerun-if-changed=src/realisation.rs");
    println!("cargo:rerun-if-changed=src/cxx/");

    let nix_main = pkg_config::probe_library("nix-main").unwrap();
    let nix_store = pkg_config::probe_library("nix-store").unwrap();
    let nix_util = pkg_config::probe_library("nix-util").unwrap();
    let libsodium = pkg_config::probe_library("libsodium").unwrap();

    cxx_build::bridges(["src/lib.rs", "src/hash.rs", "src/realisation.rs"])
        .files([
            "src/nix.cpp",
            "src/cxx/utils.cpp",
            "src/cxx/hash.cpp",
            "src/cxx/realisation.cpp",
        ])
        .flag("-std=c++23")
        .flag("-O2")
        .includes(nix_main.include_paths.clone())
        .compile("nix_utils");

    // Re-emit link directives after the static archive so the linker can
    // resolve symbols from libnix_utils.a that reference nix shared libs.
    for lib in [&nix_main, &nix_store, &nix_util, &libsodium] {
        for path in &lib.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
        for name in &lib.libs {
            println!("cargo:rustc-link-lib=dylib={name}");
        }
    }
}
