use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

const LACE_SOURCES: &[&str] = &["lace.c"];

const SYLVAN_SOURCES: &[&str] = &[
    "sha2.c",
    "sylvan_bdd.c",
    "sylvan_cache.c",
    "sylvan_common.c",
    "sylvan_hash.c",
    "sylvan_ldd.c",
    "sylvan_mt.c",
    "sylvan_mtbdd.c",
    "sylvan_platform.c",
    "sylvan_refs.c",
    "sylvan_sl.c",
    "sylvan_stats.c",
    "sylvan_table.c",
    "sylvan_zdd.c",
];

fn main() -> Result<(), String> {
    let out_dir = PathBuf::from(
        env::var("OUT_DIR")
            .map_err(|_| "Environmental variable `OUT_DIR` not defined.".to_string())?,
    );

    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR")
            .map_err(|_| "Environmental variable `CARGO_MANIFEST_DIR` not defined.".to_string())?,
    );

    let lace_dir = PathBuf::from("3rd-party/lace/src");
    let sylvan_dir = PathBuf::from("3rd-party/sylvan/src");
    let wrapper_dir = PathBuf::from("wrapper");

    if !lace_dir.join("lace.c").exists() || !sylvan_dir.join("sylvan.h").exists() {
        return Err(
            "Sylvan or Lace sources not found; run `git submodule update --init`.".to_string(),
        );
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let is_msvc = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let is_windows = env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("windows");
    // Matches CMake's `Release` configuration, which disables the many `assert`s.
    let is_release = env::var("PROFILE").as_deref() == Ok("release");
    let flag = |b: bool| if b { "1" } else { "0" }.to_string();

    // Lace and Sylvan normally generate these headers through CMake's `configure_file`.
    let lace_vars = HashMap::from([
        ("LACE_PIE_TIMES", flag(false)),
        ("LACE_COUNT_TASKS", flag(false)),
        ("LACE_COUNT_STEALS", flag(false)),
        ("LACE_COUNT_SPLITS", flag(false)),
        ("LACE_BACKOFF", flag(true)),
        ("LACE_USE_HWLOC", flag(false)),
        ("HAVE_SCHED_GETAFFINITY", flag(target_os == "linux")),
    ]);
    configure_file(
        &lace_dir.join("lace_config.h.in"),
        &out_dir.join("lace_config.h"),
        &lace_vars,
    )?;

    let (major, minor, patch) = sylvan_version(Path::new("3rd-party/sylvan/CMakeLists.txt"))?;
    let sylvan_vars = HashMap::from([
        ("PROJECT_VERSION_MAJOR", major),
        ("PROJECT_VERSION_MINOR", minor),
        ("PROJECT_VERSION_PATCH", patch),
        ("SYLVAN_STATS", flag(false)),
        // On Windows, this uses `VirtualAlloc` instead.
        ("SYLVAN_USE_MMAP", flag(true)),
        // CMake detects this at configure time, which is not possible when cross compiling.
        // Apple Silicon uses 128 byte cache lines.
        (
            "SYLVAN_CACHE_LINE_SIZE",
            if target_os == "macos" && target_arch == "aarch64" {
                "128"
            } else {
                "64"
            }
            .to_string(),
        ),
    ]);
    configure_file(
        &sylvan_dir.join("sylvan_config.h.in"),
        &out_dir.join("sylvan_config.h"),
        &sylvan_vars,
    )?;

    // Generate the Rust bindings. Sylvan defines many of its operations as `static inline`
    // functions (including the `*_RUN` entry points of Lace tasks), for which bindgen generates
    // small C wrappers in `extern.c` that are compiled below.
    let extern_path = out_dir.join("extern");
    let mut bindings = bindgen::Builder::default();
    if is_release {
        // Must match the C build, since some declarations depend on it.
        bindings = bindings.clang_arg("-DNDEBUG");
    }
    bindings
        // The header path ends up in `extern.c`, so it must be absolute.
        .header(
            manifest_dir
                .join(&wrapper_dir)
                .join("wrapper.h")
                .display()
                .to_string(),
        )
        .clang_arg(format!("-I{}", lace_dir.display()))
        .clang_arg(format!("-I{}", sylvan_dir.display()))
        .clang_arg(format!("-I{}", out_dir.display()))
        .clang_arg("-std=gnu11")
        .rust_target(bindgen::RustTarget::stable(85, 0).map_err(|e| e.to_string())?)
        .rust_edition(bindgen::RustEdition::Edition2024)
        .allowlist_function("(sylvan|mtbdd|lddmc|zdd|lace)_.*")
        .allowlist_type("(sylvan|mtbdd|lddmc|zdd|lace)_.*")
        .allowlist_var("(sylvan|mtbdd|lddmc|zdd|lace)_.*")
        // These are `static const` in the headers, so there is no symbol to link against. They
        // are defined as Rust constants instead.
        .blocklist_item("(sylvan|mtbdd|zdd)_(complement|false|true|invalid)")
        // Declared in `sylvan_zdd.h`, but not implemented.
        .blocklist_function("zdd_map_(add|addall|contains|count|remove|removeall)")
        // Returns `long double`, which has no Rust equivalent; see `lddmc_satcount` in lib.rs.
        .blocklist_function("lddmc_satcount_[A-Z_]+")
        .wrap_static_fns(true)
        .wrap_static_fns_path(&extern_path)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .parse_callbacks(Box::new(PlainTextComments))
        .generate()
        .map_err(|e| format!("Failed to generate bindings: {e}"))?
        .write_to_file(out_dir.join("bindings.rs"))
        .map_err(|e| format!("Failed to write bindings: {e}"))?;

    let mut base = cc::Build::new();
    base.include(&lace_dir)
        .include(&sylvan_dir)
        .include(&wrapper_dir)
        .include(&out_dir);

    if is_msvc {
        // The same flags as the upstream CMake configuration for MSVC.
        base.flag("/std:c17")
            .flag("/Zc:__STDC__")
            .flag("/Zc:preprocessor")
            .flag("/experimental:c11atomics");
    } else {
        base.flag("-std=gnu11").flag("-fno-strict-aliasing");
    }

    if is_release {
        base.define("NDEBUG", None);
    }

    // Lace only defines `_GNU_SOURCE` when `__GLIBC__` is set, which is never the case before
    // the first libc header is included, so `sched_getaffinity` would be undeclared.
    if target_os == "linux" {
        base.define("_GNU_SOURCE", None);
    }

    // Warnings are only shown for the wrapper; the third-party sources and the generated code
    // trigger warnings on newer compilers that we cannot fix upstream. The wrapper is compiled
    // first so that it precedes `sylvan` on the linker command line.
    base.clone()
        .file(wrapper_dir.join("wrapper.c"))
        .compile("sylvan_sys_wrapper");

    base.warnings(false)
        .file(extern_path.with_extension("c"))
        .files(SYLVAN_SOURCES.iter().map(|f| sylvan_dir.join(f)))
        .files(LACE_SOURCES.iter().map(|f| lace_dir.join(f)))
        .compile("sylvan");

    // Lace uses pthreads, except with MSVC where it uses the Windows API. On Windows, it also
    // uses `WaitOnAddress` from the synchronization library.
    if !is_msvc {
        cargo_emit::rustc_link_lib!("pthread");
    }
    if is_windows {
        cargo_emit::rustc_link_lib!("synchronization");
    }
    if !is_windows && target_os != "macos" {
        cargo_emit::rustc_link_lib!("m");
    }

    cargo_emit::rerun_if_changed!(
        "build.rs",
        "wrapper",
        "3rd-party/lace/src",
        "3rd-party/sylvan/src",
        "3rd-party/sylvan/CMakeLists.txt",
    );

    Ok(())
}

/// Renders the C doc comments as plain text. They are not valid Markdown (e.g. `<dd>` is used to
/// refer to arguments), and indented examples would otherwise be compiled as Rust doctests.
#[derive(Debug)]
struct PlainTextComments;

impl bindgen::callbacks::ParseCallbacks for PlainTextComments {
    fn process_comment(&self, comment: &str) -> Option<String> {
        Some(format!("```text\n{}\n```", comment.trim()))
    }
}

/// A minimal implementation of CMake's `configure_file`, supporting `@VAR@` substitution,
/// `#cmakedefine01 VAR` and `#cmakedefine VAR ...`.
fn configure_file(input: &Path, output: &Path, vars: &HashMap<&str, String>) -> Result<(), String> {
    let template = fs::read_to_string(input)
        .map_err(|e| format!("Failed to read {}: {e}", input.display()))?;

    let lookup = |name: &str| -> Result<&String, String> {
        vars.get(name)
            .ok_or_else(|| format!("No value for `{name}` in {}", input.display()))
    };

    let mut result = String::new();
    for line in template.lines() {
        let line = if let Some(name) = line.strip_prefix("#cmakedefine01 ") {
            let name = name.trim();
            format!("#define {name} {}", lookup(name)?)
        } else if let Some(rest) = line.strip_prefix("#cmakedefine ") {
            let name = rest.split_whitespace().next().unwrap_or_default();
            match lookup(name)?.as_str() {
                "" | "0" | "OFF" => format!("/* #undef {name} */"),
                _ => format!("#define {}", rest.trim()),
            }
        } else {
            line.to_string()
        };

        // Substitute all `@VAR@` occurrences.
        let mut parts = line.split('@');
        result.push_str(parts.next().unwrap_or_default());
        let rest: Vec<&str> = parts.collect();
        let mut i = 0;
        while i < rest.len() {
            if i + 1 < rest.len() && !rest[i].is_empty() && vars.contains_key(rest[i]) {
                result.push_str(lookup(rest[i])?);
                result.push_str(rest[i + 1]);
                i += 2;
            } else {
                result.push('@');
                result.push_str(rest[i]);
                i += 1;
            }
        }
        result.push('\n');
    }

    fs::write(output, result).map_err(|e| format!("Failed to write {}: {e}", output.display()))
}

/// Extracts the `VERSION x.y.z` of the `project(...)` declaration in a CMakeLists.txt.
fn sylvan_version(cmake_lists: &Path) -> Result<(String, String, String), String> {
    let contents = fs::read_to_string(cmake_lists)
        .map_err(|e| format!("Failed to read {}: {e}", cmake_lists.display()))?;

    let version = contents
        .lines()
        .find_map(|line| line.trim().strip_prefix("VERSION "))
        .ok_or_else(|| format!("No project version found in {}", cmake_lists.display()))?;

    let mut parts = version.trim().split('.').map(str::to_string);
    match (parts.next(), parts.next(), parts.next()) {
        (Some(major), Some(minor), Some(patch)) => Ok((major, minor, patch)),
        _ => Err(format!("Invalid project version `{version}`")),
    }
}
