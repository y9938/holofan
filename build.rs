use serde_json::Value;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

fn main() {
    println!("cargo:rerun-if-changed=ui/main.slint");
    let ui_config = slint_build::CompilerConfiguration::new().with_style("fluent-light".into());
    slint_build::compile_with_config("ui/main.slint", ui_config).expect("compile Slint UI");
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    build_jpeg8c(&manifest_dir);
    let locale_manifest = manifest_dir.join("locales.json");
    println!("cargo:rerun-if-changed={}", locale_manifest.display());

    let manifest: Value =
        serde_json::from_slice(&fs::read(&locale_manifest).expect("read locale registry"))
            .expect("parse locale registry");
    let source = manifest["source"].as_str().expect("source locale tag");
    let fallback = manifest["fallback"].as_str().expect("fallback locale tag");
    let locales = manifest["locales"].as_array().expect("locales array");
    let mut tags = HashSet::new();
    let mut definitions = String::from("&[\n");
    let mut catalogs = String::from("&[\n");

    for locale in locales {
        let tag = locale["tag"].as_str().expect("locale tag");
        let name = locale["name"].as_str().expect("locale display name");
        let flag = locale["flag"].as_str();
        assert!(tags.insert(tag), "duplicate locale tag: {tag}");
        let parsed = icu_locale_core::Locale::from_str(tag)
            .unwrap_or_else(|_| panic!("invalid BCP 47 locale tag: {tag}"));
        assert_eq!(
            parsed.to_string(),
            tag,
            "locale tag must be canonical: {tag}"
        );
        assert!(tag.len() <= 40, "locale tag is too long: {tag}");

        let catalog_path = manifest_dir.join("locales").join(format!("{tag}.json"));
        assert!(
            catalog_path.is_file(),
            "missing catalog: {}",
            catalog_path.display()
        );
        let _: Value =
            serde_json::from_slice(&fs::read(&catalog_path).expect("read locale catalog"))
                .expect("locale catalog must contain valid JSON");
        println!("cargo:rerun-if-changed={}", catalog_path.display());
        let catalog_path =
            serde_json::to_string(catalog_path.to_str().expect("catalog path is UTF-8"))
                .expect("escape catalog path");
        let tag_literal = serde_json::to_string(tag).expect("escape locale tag");
        let name_literal = serde_json::to_string(name).expect("escape locale name");
        let flag_literal = flag
            .map(|value| serde_json::to_string(value).expect("escape locale flag"))
            .map(|value| format!("Some({value})"))
            .unwrap_or_else(|| "None".to_owned());
        definitions.push_str(&format!(
            "    LocaleInfo {{ tag: {tag_literal}, name: {name_literal}, flag: {flag_literal} }},\n"
        ));
        if tag != source {
            catalogs.push_str(&format!(
                "    ({tag_literal}, include_str!({catalog_path})),\n"
            ));
        }
    }

    assert!(
        tags.contains(source),
        "source locale is not registered: {source}"
    );
    assert!(
        tags.contains(fallback),
        "fallback locale is not registered: {fallback}"
    );
    definitions.push_str("]\n");
    catalogs.push_str("]\n");
    let generated = format!(
        "pub(crate) const SOURCE_LOCALE: &str = {};\n\
         pub(crate) const FALLBACK_LOCALE: &str = {};\n\
         pub(crate) static AVAILABLE_LOCALES: &[LocaleInfo] = {};\n\
         pub(crate) static EMBEDDED_CATALOGS: &[(&str, &str)] = {};\n",
        serde_json::to_string(source).expect("escape source locale"),
        serde_json::to_string(fallback).expect("escape fallback locale"),
        definitions,
        catalogs,
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("out dir")).join("locales.rs");
    fs::write(output, generated).expect("write locale registry");

    // Embed application icon for Windows PE executables
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let icon_path = manifest_dir.join("assets").join("icon.ico");
        println!("cargo:rerun-if-changed={}", icon_path.display());
        let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("out dir"));
        let rc_path = out_dir.join("icon.rc");
        let res_path = out_dir.join("icon.res");

        let rc_content = format!(
            "1 ICON \"{}\"\n",
            icon_path.display().to_string().replace('\\', "/")
        );
        fs::write(&rc_path, rc_content).expect("write icon.rc");

        let compilers = ["llvm-rc", "rc", "x86_64-w64-mingw32-windres", "windres"];
        let mut compiled = false;
        for compiler in compilers {
            if compiler == "windres" || compiler.ends_with("-windres") {
                let status = std::process::Command::new(compiler)
                    .arg("-i")
                    .arg(&rc_path)
                    .arg("-O")
                    .arg("coff")
                    .arg("-o")
                    .arg(&res_path)
                    .status();
                if let Ok(s) = status {
                    if s.success() {
                        compiled = true;
                        break;
                    }
                }
            } else {
                let status = std::process::Command::new(compiler)
                    .arg(format!("/fo{}", res_path.display()))
                    .arg(&rc_path)
                    .status();
                if let Ok(s) = status {
                    if s.success() {
                        compiled = true;
                        break;
                    }
                }
            }
        }
        if compiled {
            println!("cargo:rustc-link-arg={}", res_path.display());
        }
    }
}

fn build_jpeg8c(manifest_dir: &std::path::Path) {
    let source_dir = manifest_dir.join("vendor/jpeg8c");
    let sources = [
        "jaricom.c",
        "jcomapi.c",
        "jdapimin.c",
        "jdapistd.c",
        "jdarith.c",
        "jdatasrc.c",
        "jdcoefct.c",
        "jdcolor.c",
        "jddctmgr.c",
        "jdhuff.c",
        "jdinput.c",
        "jdmainct.c",
        "jdmarker.c",
        "jdmaster.c",
        "jdmerge.c",
        "jdpostct.c",
        "jdsample.c",
        "jerror.c",
        "jidctflt.c",
        "jidctfst.c",
        "jidctint.c",
        "jquant1.c",
        "jquant2.c",
        "jutils.c",
        "jmemmgr.c",
        "jmemnobs.c",
        "src/jpeg8c_wrapper.c",
    ];

    let mut build = cc::Build::new();
    build.include(&source_dir).warnings(false);
    for source in sources {
        let path = if source.starts_with("src/") {
            manifest_dir.join(source)
        } else {
            source_dir.join(source)
        };
        println!("cargo:rerun-if-changed={}", path.display());
        build.file(path);
    }
    for header in [
        "README.holofan.md",
        "jconfig.h",
        "jerror.h",
        "jinclude.h",
        "jmemsys.h",
        "jmorecfg.h",
        "jpegint.h",
        "jpeglib.h",
        "jversion.h",
        "jdct.h",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            source_dir.join(header).display()
        );
    }
    build.compile("jpeg8c");
}
