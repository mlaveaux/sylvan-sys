use std::env;
use std::fs;
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
    "sylvan_refs.c",
    "sylvan_sl.c",
    "sylvan_stats.c",
    "sylvan_table.c",
    "sylvan_zdd.c",
];

fn main() -> Result<(), String> {
    let build_sylvan = env::var_os("CARGO_FEATURE_BUILD_SYLVAN").is_some();
    if !build_sylvan {
        // If silent build is active, don't do anything.
        return Ok(());
    }

    let out_dir = PathBuf::from(
        env::var("OUT_DIR")
            .map_err(|_| "Environmental variable `OUT_DIR` not defined.".to_string())?,
    );

    let lace_dir = PathBuf::from("3rd-party/lace/src");
    let sylvan_dir = PathBuf::from("3rd-party/sylvan/src");
    let wrapper_dir = PathBuf::from("wrapper/src");

    if !lace_dir.join("lace.c").exists() || !sylvan_dir.join("sylvan.h").exists() {
        return Err(
            "Sylvan or Lace sources not found; run `git submodule update --init`.".to_string(),
        );
    }

    // Lace normally generates this header through CMake's `configure_file`.
    let use_mmap = env::var("CARGO_CFG_UNIX").is_ok();
    let lace_config = format!(
        "#define LACE_PIE_TIMES 0\n\
         #define LACE_COUNT_TASKS 0\n\
         #define LACE_COUNT_STEALS 0\n\
         #define LACE_COUNT_SPLITS 0\n\
         #define LACE_USE_HWLOC 0\n\
         #define LACE_USE_MMAP {}\n",
        use_mmap as u8
    );
    fs::write(out_dir.join("lace_config.h"), lace_config)
        .map_err(|e| format!("Failed to write lace_config.h: {e}"))?;

    let is_debug = env::var("DEBUG").map(|v| v == "true").unwrap_or(true);

    let mut base = cc::Build::new();
    base.flag_if_supported("-std=gnu11")
        .include(&lace_dir)
        .include(&sylvan_dir)
        .include(&wrapper_dir)
        .include(&out_dir)
        .flag_if_supported("-fno-strict-aliasing");

    // The wrapper is compiled with `-Werror` in debug builds. The third-party sources are not,
    // since newer compilers introduce warnings that we cannot fix upstream. The wrapper is
    // compiled first so that it precedes `sylvan` on the linker command line.
    let mut wrapper = base.clone();
    if is_debug {
        wrapper.flag_if_supported("-Werror");
    }
    wrapper
        .file(wrapper_dir.join("sylvan-wrapper.c"))
        .compile("wrapper");

    base.warnings(false)
        .files(SYLVAN_SOURCES.iter().map(|f| sylvan_dir.join(f)))
        .files(LACE_SOURCES.iter().map(|f| lace_dir.join(f)))
        .compile("sylvan");

    cargo_emit::rustc_link_lib!("m", "pthread");
    cargo_emit::rerun_if_changed!(
        "build.rs",
        "wrapper/src",
        "3rd-party/lace/src",
        "3rd-party/sylvan/src",
    );

    Ok(())
}
