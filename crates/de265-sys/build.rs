//! Compile the bundled libde265 with `cc` (no CMake, no system install).

use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.extension().is_some_and(|e| e == "cc") {
            out.push(p);
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("libde265");
    let lib = root.join("libde265");
    println!("cargo:rerun-if-changed=libde265");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let x86 = arch == "x86_64" || arch == "x86";

    let base = |b: &mut cc::Build| {
        b.cpp(true)
            .include(&root)
            .include(&lib)
            .define("LIBDE265_STATIC_BUILD", None)
            .define("DE265_LOG_ERROR", None)
            .warnings(false)
            .opt_level(3);
        if !b.get_compiler().is_like_msvc() {
            b.std("c++11");
        }
        if target_os == "windows" {
            b.define("HAVE_MALLOC_H", None).define("NOMINMAX", None).define("_CRT_SECURE_NO_WARNINGS", None);
        } else {
            b.define("HAVE_POSIX_MEMALIGN", None);
        }
        if x86 {
            b.define("HAVE_SSE4_1", "1");
        }
    };

    let mut files = Vec::new();
    sources(&lib, &mut files);
    sources(&lib.join("encoder"), &mut files);
    sources(&lib.join("encoder").join("algo"), &mut files);

    // MSVC 14.51 (Visual Studio 18) miscompiles scan.cc at -O2: the scan-position
    // search in `init_scan_orders` never ends and runs off its table. It only
    // fills lookup tables once at start-up, so it is built without optimisation.
    let (slow, fast): (Vec<_>, Vec<_>) = files.into_iter().partition(|f| f.file_name().is_some_and(|n| n == "scan.cc"));

    let mut build = cc::Build::new();
    base(&mut build);
    build.files(&fast);

    let mut scan = cc::Build::new();
    base(&mut scan);
    if scan.get_compiler().is_like_msvc() {
        scan.opt_level(0);
    }
    scan.files(&slow);
    for obj in scan.compile_intermediates() {
        build.object(obj);
    }

    if x86 {
        // The SSE kernels are picked at run time (cpuid), so only they get SSE4.1 flags.
        let mut sse = cc::Build::new();
        base(&mut sse);
        if !sse.get_compiler().is_like_msvc() {
            sse.flag("-msse4.1");
        }
        for f in ["sse.cc", "sse-motion.cc", "sse-dct.cc"] {
            sse.file(lib.join("x86").join(f));
        }
        for obj in sse.compile_intermediates() {
            build.object(obj);
        }
    }
    if target_os == "windows" {
        let mut c = cc::Build::new();
        c.file(root.join("extra").join("win32cond.c")).warnings(false);
        for obj in c.compile_intermediates() {
            build.object(obj);
        }
    }
    build.compile("de265");
    if target_os != "windows" {
        println!("cargo:rustc-link-lib=pthread");
    }
}
