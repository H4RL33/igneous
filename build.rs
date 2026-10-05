//! Generates `config.rs`, compiles the Blueprint UI files and bundles them with
//! the other resources, and compiles the GSettings schema so the app runs
//! straight from `cargo run`. Meson sets the IGNEOUS_* variables (app ID,
//! profile, version, locale directory, and that the build will be installed)
//! for installed builds.

use std::path::{Path, PathBuf};
use std::process::Command;

const BASE_ID: &str = "dev.h4rl3y.igneous";

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let debug = std::env::var("PROFILE").as_deref() == Ok("debug");
    let app_id = env("IGNEOUS_APP_ID").unwrap_or_else(|| {
        if debug {
            format!("{BASE_ID}.Devel")
        } else {
            BASE_ID.to_owned()
        }
    });
    let profile = env("IGNEOUS_PROFILE")
        .unwrap_or_else(|| if debug { "Devel".into() } else { String::new() });
    let version =
        env("IGNEOUS_VERSION").unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap());
    let localedir = env("IGNEOUS_LOCALEDIR").unwrap_or_else(|| "/usr/share/locale".to_owned());
    // Installed builds use the installed schema; leaving the build tree's
    // path out keeps it from being baked into packaged binaries.
    let schema_dir = if env("IGNEOUS_INSTALLED").is_some() {
        String::new()
    } else {
        out.join("schemas").display().to_string()
    };

    std::fs::write(
        out.join("config.rs"),
        format!(
            "pub const APP_ID: &str = {app_id:?};\n\
             pub const PROFILE: &str = {profile:?};\n\
             pub const VERSION: &str = {version:?};\n\
             pub const GETTEXT_PACKAGE: &str = \"igneous\";\n\
             /// Where compiled translations are installed.\n\
             pub const LOCALEDIR: &str = {localedir:?};\n\
             pub const RESOURCE_BASE: &str = \"/dev/h4rl3y/igneous\";\n\
             /// Compiled schema for runs that aren't installed (empty when\n\
             /// built to be installed).\n\
             pub const SCHEMA_DIR: &str = {schema_dir:?};\n",
        ),
    )
    .unwrap();

    compile_blueprints(&out.join("ui"));
    glib_build_tools::compile_resources(
        &[
            out.join("ui"),
            PathBuf::from("data/resources"),
            PathBuf::from("data"),
        ],
        "data/resources/resources.gresource.xml",
        "igneous.gresource",
    );
    compile_schema(&out.join("schemas"), &app_id);

    println!("cargo:rerun-if-changed=data");
    for var in [
        "IGNEOUS_APP_ID",
        "IGNEOUS_PROFILE",
        "IGNEOUS_VERSION",
        "IGNEOUS_LOCALEDIR",
        "IGNEOUS_INSTALLED",
        "BLUEPRINT_COMPILER",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn compile_blueprints(out: &Path) {
    std::fs::create_dir_all(out).unwrap();
    let mut blueprints: Vec<PathBuf> = std::fs::read_dir("data/resources/ui")
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "blp"))
        .collect();
    blueprints.sort();
    let output = blueprint_compiler()
        .arg("batch-compile")
        .arg(out)
        .arg("data/resources/ui")
        .args(&blueprints)
        .output()
        .expect("failed to run blueprint-compiler");
    if !output.status.success() {
        panic!(
            "blueprint-compiler failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `$BLUEPRINT_COMPILER`, then `blueprint-compiler` on PATH, then the copy
/// Meson downloads into `subprojects/`.
fn blueprint_compiler() -> Command {
    if let Some(path) = env("BLUEPRINT_COMPILER") {
        return Command::new(path);
    }
    if Command::new("blueprint-compiler")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return Command::new("blueprint-compiler");
    }
    let vendored = Path::new("subprojects/blueprint-compiler/blueprint-compiler.py");
    if vendored.exists() {
        let mut command = Command::new("python3");
        command.arg(vendored);
        return command;
    }
    panic!(
        "blueprint-compiler not found. Install it, or run \
         `meson subprojects download blueprint-compiler` in the source tree."
    );
}

fn compile_schema(out: &Path, app_id: &str) {
    std::fs::create_dir_all(out).unwrap();
    let schema = std::fs::read_to_string("data/dev.h4rl3y.igneous.gschema.xml.in")
        .unwrap()
        .replace("@APP_ID@", app_id)
        .replace("@SCHEMA_PATH@", &format!("/{}/", app_id.replace('.', "/")));
    std::fs::write(out.join(format!("{app_id}.gschema.xml")), schema).unwrap();
    let status = Command::new("glib-compile-schemas")
        .arg("--strict")
        .arg(out)
        .status()
        .expect("failed to run glib-compile-schemas");
    assert!(status.success(), "glib-compile-schemas failed");
}
